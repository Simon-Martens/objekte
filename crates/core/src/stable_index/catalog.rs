//! Physical-file, record, path and identity catalogs.

use roaring::RoaringBitmap;

use crate::inventory::{FileEntry, RecordClass, RecordEntry};
use crate::model::{CollectionId, FileId, RecordId, RecordSlot};

use super::codec::{
    self, ComponentKey, FileCatalogRecord, PathCatalogEntry, PathKey, RecordCatalogRecord,
    RecordRef,
};
use super::{StableIndexError, StablePrototype};

impl StablePrototype {
    pub fn file_by_id(&mut self, file: FileId) -> Result<Option<FileEntry>, StableIndexError> {
        Ok(self
            .file_catalog_record(file)?
            .filter(|record| record.live)
            .map(|record| record.entry))
    }

    pub fn file_catalog_record(
        &mut self,
        file: FileId,
    ) -> Result<Option<FileCatalogRecord>, StableIndexError> {
        let Some(component) = self.file_catalog_component() else {
            return Err(StableIndexError::MissingDependency(
                "physical-file catalog".into(),
            ));
        };
        let tree = self.tree(component)?;
        let value = tree.get(&codec::file_id_key(file));
        self.counters.stable_gets += 1;
        let Some(value) = value else {
            return Ok(None);
        };
        codec::decode_meta(&value)
            .map(Some)
            .map_err(StableIndexError::Format)
    }

    pub fn record_catalog_entry(
        &mut self,
        collection: CollectionId,
        slot: RecordSlot,
    ) -> Result<Option<RecordCatalogRecord>, StableIndexError> {
        let record_component = self
            .collections
            .get(&collection)
            .ok_or_else(|| StableIndexError::Format("unknown collection".into()))?
            .records;
        let tree = self.tree(record_component)?;
        let value = tree.get(&codec::record_slot_key(slot));
        self.counters.stable_gets += 1;
        let Some(value) = value else {
            return Ok(None);
        };
        codec::decode_meta(&value)
            .map(Some)
            .map_err(StableIndexError::Format)
    }

    pub fn record_by_slot(
        &mut self,
        collection: CollectionId,
        slot: RecordSlot,
    ) -> Result<Option<RecordEntry>, StableIndexError> {
        Ok(self
            .record_catalog_entry(collection, slot)?
            .filter(|record| record.live)
            .map(|record| record.entry))
    }

    pub fn records_for_path(
        &mut self,
        path: &std::path::Path,
    ) -> Result<Vec<RecordRef>, StableIndexError> {
        let key = PathKey::from_path(path).map_err(StableIndexError::Format)?;
        let Some(component) = self.path_catalog_component() else {
            return Err(StableIndexError::MissingDependency(
                "path lookup catalog".into(),
            ));
        };
        let tree = self.tree(component)?;
        let Some(value) = tree.get(&ComponentKey::Path(key)) else {
            return Ok(Vec::new());
        };
        let entry: PathCatalogEntry =
            codec::decode_meta(&value).map_err(StableIndexError::Format)?;
        Ok(entry.refs)
    }

    pub fn path_entry(
        &mut self,
        key: &PathKey,
    ) -> Result<Option<PathCatalogEntry>, StableIndexError> {
        let Some(component) = self.path_catalog_component() else {
            return Err(StableIndexError::MissingDependency(
                "path lookup catalog".into(),
            ));
        };
        let tree = self.tree(component)?;
        let value = tree.get(&ComponentKey::Path(key.clone()));
        self.counters.stable_gets += 1;
        value
            .map(|value| codec::decode_meta(&value).map_err(StableIndexError::Format))
            .transpose()
    }

    pub fn records_for_path_key(
        &mut self,
        key: &PathKey,
    ) -> Result<Vec<RecordRef>, StableIndexError> {
        Ok(self
            .path_entry(key)?
            .map_or_else(Vec::new, |entry| entry.refs))
    }

    pub fn identities_for_path(
        &mut self,
        path: &std::path::Path,
    ) -> Result<Vec<(CollectionId, RecordId)>, StableIndexError> {
        let refs = self.records_for_path(path)?;
        let mut identities = Vec::with_capacity(refs.len());
        for reference in refs {
            if let Some(record) = self.record_by_slot(reference.collection, reference.slot)? {
                identities.push((reference.collection, record.id));
            }
        }
        Ok(identities)
    }

    pub fn records_for_identity(
        &mut self,
        collection: CollectionId,
        id: &RecordId,
    ) -> Result<Vec<RecordSlot>, StableIndexError> {
        let identity_component = self
            .collections
            .get(&collection)
            .ok_or_else(|| StableIndexError::Format("unknown collection".into()))?
            .identities;
        let tree = self.tree(identity_component)?;
        let value = tree.get(&codec::identity_key(id));
        self.counters.stable_gets += 1;
        let Some(value) = value else {
            return Ok(Vec::new());
        };
        let slots = super::memory::decode_bitmap(&value)?;
        self.counters.bitmap_decodes += 1;
        self.filter_live(collection, &slots)
    }

    pub fn identities_for_file(
        &mut self,
        file: FileId,
    ) -> Result<Vec<(CollectionId, RecordId)>, StableIndexError> {
        let file_record = match self.file_catalog_record(file)? {
            Some(record) if record.live => record,
            _ => return Ok(Vec::new()),
        };
        let mut identities = Vec::new();
        for collection in file_record.entry.collections.clone() {
            let records_component = self
                .collections
                .get(&collection)
                .ok_or_else(|| StableIndexError::Format("unknown collection".into()))?
                .records;
            let tree = self.tree(records_component)?;
            for entry in tree.iter() {
                let ComponentKey::RecordSlot(_) = entry.key() else {
                    continue;
                };
                let record: RecordCatalogRecord =
                    codec::decode_meta(&entry.value()).map_err(StableIndexError::Format)?;
                if record.live
                    && record.entry.file == file
                    && record.entry.class == RecordClass::Logical
                {
                    identities.push((collection, record.entry.id));
                }
            }
        }
        Ok(identities)
    }

    pub fn live_slots(
        &mut self,
        collection: CollectionId,
    ) -> Result<RoaringBitmap, StableIndexError> {
        self.live_domain_owned(collection)
    }

    pub(crate) fn filter_live(
        &mut self,
        collection: CollectionId,
        slots: &RoaringBitmap,
    ) -> Result<Vec<RecordSlot>, StableIndexError> {
        let live = self.live_domain_shared(collection)?;
        Ok((slots & &*live).iter().map(RecordSlot).collect::<Vec<_>>())
    }
}
