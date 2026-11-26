use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use crate::index::storage::order::key_transfer::transfer_order_keys;
use crate::index::storage::order::{
    DeclaredOrderDefinition, DeclaredOrderStore, DeclaredOrderWriter, ORDER_KEY_BATCH_SIZE,
    OrderColumnReader, OrderIdentities,
};
use crate::index::storage::transfer::{StorageError, StorageResult};
use crate::model::{FieldPath, RecordSlot};
use crate::records::RecordIdSlot;

use super::{SortCollection, SortConfig, SortKey};

#[derive(Clone)]
pub(in crate::index::memory) struct PlacedOrder {
    reader: Arc<dyn OrderColumnReader>,
    store: Arc<dyn DeclaredOrderStore>,
}

impl fmt::Debug for PlacedOrder {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PlacedOrder")
            .field("path", self.reader.path())
            .field("slots", &self.reader.key_count())
            .finish()
    }
}

pub(in crate::index::memory) struct PlacedOrderWriter {
    store: Arc<dyn DeclaredOrderStore>,
    definition: DeclaredOrderDefinition,
    writer: Box<dyn DeclaredOrderWriter>,
    keys: usize,
}

impl PlacedOrderWriter {
    pub(super) fn new(
        store: Arc<dyn DeclaredOrderStore>,
        definition: DeclaredOrderDefinition,
    ) -> StorageResult<Self> {
        let writer = store.start(definition.clone())?;
        Ok(Self {
            store,
            definition,
            writer,
            keys: 0,
        })
    }

    pub(super) fn append(&mut self, base: u32, keys: &[SortKey]) -> StorageResult<()> {
        if self.keys != base as usize {
            return Err(StorageError::invalid(
                "write declared order",
                "keys are not in slot order",
            ));
        }
        for batch in keys.chunks(ORDER_KEY_BATCH_SIZE) {
            let base = u32::try_from(self.keys).map_err(|_| {
                StorageError::invalid("write declared order", "slot count overflow")
            })?;
            self.writer.push_keys(RecordSlot(base), batch)?;
            self.keys += batch.len();
        }
        Ok(())
    }

    pub(super) fn finish(self, records: &dyn OrderIdentities) -> StorageResult<PlacedOrder> {
        const OP: &str = "seal declared sort column";
        let definition = self.definition;
        let count = self.keys;
        if count != records.len() {
            return Err(StorageError::invalid(OP, "key coverage mismatch"));
        }
        let reader = self.writer.seal(records)?;
        if reader.path() != &definition.path
            || reader.kind() != &definition.kind
            || reader.locale() != definition.config.locale
            || reader.options() != definition.config.options
            || reader.collation_version() != definition.collation_version
            || reader.key_count() != count
            || reader.slots(false)?.len() != count
            || reader.slots(true)?.len() != count
        {
            return Err(StorageError::invalid(OP, "incompatible order returned"));
        }
        Ok(PlacedOrder {
            reader,
            store: self.store,
        })
    }
}

impl PlacedOrder {
    fn definition(&self) -> DeclaredOrderDefinition {
        DeclaredOrderDefinition {
            path: self.reader.path().clone(),
            kind: self.reader.kind().clone(),
            config: SortConfig {
                locale: self.reader.locale().to_owned(),
                options: self.reader.options(),
            },
            collation_version: self.reader.collation_version().to_owned(),
        }
    }

    pub(super) fn reader(&self) -> &dyn OrderColumnReader {
        self.reader.as_ref()
    }

    pub(super) fn rewrite<R: RecordIdSlot>(
        &self,
        records: &[R],
        mut replacement: impl FnMut(&R) -> Result<Option<SortKey>, String>,
    ) -> Result<Self, String> {
        let mut writer = PlacedOrderWriter::new(Arc::clone(&self.store), self.definition())?;
        let mut keys = Vec::with_capacity(ORDER_KEY_BATCH_SIZE);
        for batch in records.chunks(ORDER_KEY_BATCH_SIZE) {
            for record in batch {
                let key = match replacement(record)? {
                    Some(key) => key,
                    None => self.reader.key(record.record_slot())?.into_owned(),
                };
                keys.push(key);
            }
            writer.append(writer.keys as u32, &keys)?;
            keys.clear();
        }
        Ok(writer.finish(&records)?)
    }

    pub(in crate::index::memory) fn project(
        &self,
        records: &[impl RecordIdSlot],
        previous_slot: impl Fn(RecordSlot) -> Option<RecordSlot>,
    ) -> Result<Self, String> {
        let slots = records.iter().map(|record| {
            previous_slot(record.record_slot()).ok_or_else(|| {
                StorageError::invalid("project declared order", "original slot missing")
            })
        });
        let mut writer = PlacedOrderWriter::new(Arc::clone(&self.store), self.definition())?;
        writer.transfer(self.reader(), slots)?;
        Ok(writer.finish(&records)?)
    }
}

impl PlacedOrderWriter {
    fn transfer(
        &mut self,
        reader: &dyn OrderColumnReader,
        slots: impl IntoIterator<Item = StorageResult<RecordSlot>>,
    ) -> StorageResult<()> {
        let mut keys = Vec::with_capacity(ORDER_KEY_BATCH_SIZE);
        transfer_order_keys(reader, slots, &|| false, |key| {
            keys.push(key);
            if keys.len() == ORDER_KEY_BATCH_SIZE {
                self.append(self.keys as u32, &keys)?;
                keys.clear();
            }
            Ok(())
        })?;
        self.append(self.keys as u32, &keys)
    }
}

impl SortCollection {
    pub(crate) fn readers(&self) -> impl Iterator<Item = &dyn OrderColumnReader> {
        self.columns
            .values()
            .map(|column| column as &dyn OrderColumnReader)
            .chain(self.placed.values().map(PlacedOrder::reader))
    }

    pub(crate) fn column_count(&self) -> usize {
        self.columns.len() + self.placed.len()
    }

    pub(crate) fn placed_bytes(&self) -> usize {
        self.placed
            .values()
            .map(|placed| placed.reader.retained_bytes())
            .sum()
    }

    pub(crate) fn reader(&self, path: &FieldPath) -> Option<&dyn OrderColumnReader> {
        self.columns
            .get(path)
            .map(|column| column as &dyn OrderColumnReader)
            .or_else(|| self.placed.get(path).map(PlacedOrder::reader))
    }

    pub(in crate::index::memory) fn stored_key(
        &self,
        path: &FieldPath,
        slot: RecordSlot,
    ) -> StorageResult<Option<Cow<'_, SortKey>>> {
        if let Some(native) = self.columns.get(path) {
            return native.key(slot).map(Some);
        }
        self.placed
            .get(path)
            .map(|placed| placed.reader.key(slot))
            .transpose()
    }

    pub(in crate::index::memory) fn consolidation_columns(
        &self,
        shards: &[&Self],
    ) -> (Self, HashMap<FieldPath, PlacedOrder>) {
        let mut placed = HashMap::new();
        for shard in shards {
            for (path, order) in &shard.placed {
                placed.entry(path.clone()).or_insert_with(|| order.clone());
            }
        }
        let columns = self
            .columns
            .iter()
            .filter(|(path, _)| !placed.contains_key(*path))
            .map(|(path, column)| (path.clone(), column.clone()))
            .collect();
        (
            Self {
                columns,
                composites: self.composites.clone(),
                placed: HashMap::new(),
            },
            placed,
        )
    }

    pub(in crate::index::memory) fn append_consolidation_columns(
        &mut self,
        source: &Self,
        placed: &HashMap<FieldPath, PlacedOrder>,
    ) -> Result<(), String> {
        for (path, column) in &source.columns {
            if placed.contains_key(path) {
                continue;
            }
            let target = self.columns.get_mut(path).ok_or("sort column missing")?;
            if column.kind != target.kind
                || column.locale != target.locale
                || column.options != target.options
                || column.collation_version != target.collation_version
            {
                return Err("sort column representation mismatch".into());
            }
            target.keys.extend_from_slice(&column.keys);
        }
        Ok(())
    }

    pub(in crate::index::memory) fn consolidate_placed(
        &mut self,
        placed: HashMap<FieldPath, PlacedOrder>,
        shards: &[&Self],
        records: &[impl RecordIdSlot],
    ) -> Result<(), String> {
        for (path, order) in placed {
            let mut writer = PlacedOrderWriter::new(Arc::clone(&order.store), order.definition())?;
            for shard in shards {
                let reader = shard.reader(&path).ok_or("placed sort column missing")?;
                let canonical = order.reader();
                if reader.kind() != canonical.kind()
                    || reader.locale() != canonical.locale()
                    || reader.options() != canonical.options()
                    || reader.collation_version() != canonical.collation_version()
                {
                    return Err("sort column representation mismatch".into());
                }
                let slots = (0..reader.key_count()).map(|slot| Ok(RecordSlot(slot as u32)));
                writer.transfer(reader, slots)?;
            }
            self.placed.insert(path, writer.finish(&records)?);
        }
        Ok(())
    }
}
