use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use crate::index::storage::order::{DeclaredOrderRows, DeclaredOrderStore, OrderColumnReader};
use crate::index::storage::transfer::{StorageError, StorageResult};
use crate::model::{FieldPath, PathRoot, RecordSlot};
use crate::records::RecordIdSlot;

use super::{SortCollection, SortColumn, SortConfig, SortKey};

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

impl PlacedOrder {
    fn seal(store: Arc<dyn DeclaredOrderStore>, column: SortColumn) -> StorageResult<Self> {
        let path = column.path;
        let kind = column.kind;
        let config = SortConfig {
            locale: column.locale,
            options: column.options,
        };
        let version = column.collation_version;
        let count = column.keys.len();
        let reader = store.seal(DeclaredOrderRows {
            path: path.clone(),
            kind: kind.clone(),
            config: config.clone(),
            collation_version: version.clone(),
            keys: column.keys,
            ascending: column.ascending,
            descending: column.descending,
        })?;
        if reader.path() != &path
            || reader.kind() != &kind
            || reader.locale() != config.locale
            || reader.options() != config.options
            || reader.collation_version() != version
            || reader.key_count() != count
            || reader.slots(false)?.len() != count
            || reader.slots(true)?.len() != count
        {
            return Err(StorageError::invalid(
                "seal declared sort column",
                "incompatible order returned",
            ));
        }
        Ok(Self { reader, store })
    }

    fn reader(&self) -> &dyn OrderColumnReader {
        self.reader.as_ref()
    }

    fn column_with_keys(&self, keys: Vec<SortKey>) -> SortColumn {
        SortColumn {
            path: self.reader.path().clone(),
            kind: self.reader.kind().clone(),
            keys,
            ascending: Vec::new(),
            descending: Vec::new(),
            locale: self.reader.locale().to_owned(),
            options: self.reader.options(),
            collation_version: self.reader.collation_version().to_owned(),
        }
    }

    pub(in crate::index::memory) fn project(
        &self,
        records: &[impl RecordIdSlot],
        previous_slot: impl Fn(RecordSlot) -> Option<RecordSlot>,
    ) -> Result<Self, String> {
        let keys = records
            .iter()
            .map(|record| {
                let previous =
                    previous_slot(record.record_slot()).ok_or("original sort slot missing")?;
                self.reader
                    .key(previous)
                    .map(|key| key.into_owned())
                    .map_err(String::from)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut column = self.column_with_keys(keys);
        column.build_order(records)?;
        Ok(Self::seal(Arc::clone(&self.store), column)?)
    }

    fn keys(&self) -> impl Iterator<Item = StorageResult<SortKey>> {
        (0..self.reader.key_count()).map(|slot| {
            self.reader
                .key(RecordSlot(slot as u32))
                .map(Cow::into_owned)
        })
    }
}

impl SortCollection {
    pub(in crate::index::memory) fn place_columns(
        &mut self,
        store: Arc<dyn DeclaredOrderStore>,
    ) -> StorageResult<()> {
        let place = self.columns.extract_if(|path, column| {
            path.root == PathRoot::Data
                && store.accepts(path, &column.kind)
                && !self
                    .composites
                    .iter()
                    .any(|order| order.fields.iter().any(|field| &field.path == path))
        });
        for (path, column) in place {
            self.placed
                .insert(path, PlacedOrder::seal(Arc::clone(&store), column)?);
        }
        Ok(())
    }

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
    ) -> Result<(Self, HashMap<FieldPath, Arc<dyn DeclaredOrderStore>>), String> {
        let mut sorting = Self {
            columns: self.columns.clone(),
            composites: self.composites.clone(),
            placed: HashMap::new(),
        };
        let mut stores = HashMap::with_capacity(self.placed.len());
        for (path, placed) in &self.placed {
            sorting.columns.insert(
                path.clone(),
                placed.column_with_keys(placed.keys().collect::<StorageResult<_>>()?),
            );
            stores.insert(path.clone(), Arc::clone(&placed.store));
        }
        Ok((sorting, stores))
    }

    pub(in crate::index::memory) fn append_consolidation_columns(
        &mut self,
        source: &Self,
        stores: &mut HashMap<FieldPath, Arc<dyn DeclaredOrderStore>>,
    ) -> Result<(), String> {
        for (path, column) in &source.columns {
            self.check_consolidation_column(path, column)?
                .keys
                .extend_from_slice(&column.keys);
        }
        for (path, placed) in &source.placed {
            let keys = &mut self.check_consolidation_column(path, placed.reader())?.keys;
            for key in placed.keys() {
                keys.push(key?);
            }
            stores
                .entry(path.clone())
                .or_insert_with(|| Arc::clone(&placed.store));
        }
        Ok(())
    }

    fn check_consolidation_column(
        &mut self,
        path: &FieldPath,
        source: &dyn OrderColumnReader,
    ) -> Result<&mut SortColumn, String> {
        let column = self.columns.get_mut(path).ok_or("sort column missing")?;
        if &column.kind != source.kind()
            || column.locale != source.locale()
            || column.options != source.options()
            || column.collation_version != source.collation_version()
        {
            return Err("sort column representation mismatch".into());
        }
        Ok(column)
    }

    pub(in crate::index::memory) fn restore_placement(
        &mut self,
        stores: HashMap<FieldPath, Arc<dyn DeclaredOrderStore>>,
    ) -> Result<(), String> {
        for (path, store) in stores {
            let column = self
                .columns
                .remove(&path)
                .ok_or("placed sort column missing")?;
            self.placed.insert(path, PlacedOrder::seal(store, column)?);
        }
        Ok(())
    }
}
