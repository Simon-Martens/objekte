//! Exclusive primitive mutation and fixture-level file lifecycle.

use std::collections::BTreeMap;

use roaring::RoaringBitmap;

use crate::inventory::{FileEntry, RecordClass, RecordEntry};
use crate::model::{CollectionId, FileId, RecordId, RecordSlot};
use crate::posting::ScalarKey;

use super::codec::{
    self, ComponentKey, DOMAIN_ALL, DOMAIN_LIVE, DOMAIN_LOGICAL, DOMAIN_RAW_FALLBACK,
    PathCatalogEntry, RecordCatalogRecord, RecordRef,
};
use super::memory::{self, StoredBytes};
use super::{ComponentId, StableIndexError, StablePrototype, StandaloneIndex};

impl StablePrototype {
    pub fn next_file_id(&mut self) -> Result<FileId, StableIndexError> {
        let component = self
            .file_catalog_component()
            .ok_or_else(|| StableIndexError::MissingDependency("physical-file catalog".into()))?;
        let tree = self.tree(component)?;
        let next = tree
            .iter()
            .filter_map(|entry| match entry.key() {
                ComponentKey::FileId(id) => Some(id.saturating_add(1)),
                _ => None,
            })
            .max()
            .unwrap_or(0);
        Ok(FileId(next))
    }

    pub fn update_typed_posting(
        &mut self,
        collection: CollectionId,
        field: &super::FieldComponentKey,
        value: &ScalarKey,
        add: &RoaringBitmap,
        remove: &RoaringBitmap,
    ) -> Result<bool, StableIndexError> {
        let component = self
            .field_components(collection, field)
            .and_then(|entry| entry.values)
            .ok_or_else(|| StableIndexError::MissingDependency("typed value index".into()))?;
        self.update_posting(
            collection,
            component,
            ComponentKey::Scalar(value.clone()),
            add,
            remove,
        )
    }

    pub fn update_length_posting(
        &mut self,
        collection: CollectionId,
        field: &super::FieldComponentKey,
        length: &crate::model::Number,
        add: &RoaringBitmap,
        remove: &RoaringBitmap,
    ) -> Result<bool, StableIndexError> {
        let component = self
            .field_components(collection, field)
            .and_then(|entry| entry.lengths)
            .ok_or_else(|| StableIndexError::MissingDependency("length index".into()))?;
        self.update_posting(
            collection,
            component,
            ComponentKey::Length(length.clone()),
            add,
            remove,
        )
    }

    pub fn update_state_posting(
        &mut self,
        collection: CollectionId,
        field: &super::FieldComponentKey,
        state: u8,
        add: &RoaringBitmap,
        remove: &RoaringBitmap,
    ) -> Result<bool, StableIndexError> {
        let component = self
            .field_components(collection, field)
            .and_then(|entry| entry.states)
            .ok_or_else(|| StableIndexError::MissingDependency("state index".into()))?;
        self.update_posting(
            collection,
            component,
            ComponentKey::State(state),
            add,
            remove,
        )
    }

    pub fn update_gram_posting(
        &mut self,
        collection: CollectionId,
        field: &super::TextComponentKey,
        gram: u64,
        skip: bool,
        add: &RoaringBitmap,
        remove: &RoaringBitmap,
    ) -> Result<bool, StableIndexError> {
        let component = self
            .text_components(collection, field)
            .and_then(|entry| if skip { entry.skip } else { entry.normal })
            .ok_or_else(|| StableIndexError::MissingDependency("text gram index".into()))?;
        self.update_posting(collection, component, ComponentKey::Gram(gram), add, remove)
    }

    pub fn update_posting(
        &mut self,
        collection: CollectionId,
        component: ComponentId,
        key: ComponentKey,
        add: &RoaringBitmap,
        remove: &RoaringBitmap,
    ) -> Result<bool, StableIndexError> {
        let _ = collection;
        let current = {
            let tree = self.tree(component)?;
            tree.get(&key)
                .map(|value| memory::decode_bitmap(&value))
                .transpose()
                .map_err(StableIndexError::Format)?
                .unwrap_or_default()
        };
        let mut updated = current;
        updated |= add;
        updated -= remove;
        let tree = self.tree_mut(component)?;
        let removed = if updated.is_empty() {
            tree.remove(&key).is_some()
        } else {
            let value = memory::bitmap_value(&updated).map_err(StableIndexError::Format)?;
            tree.insert(key, value);
            false
        };
        self.counters.bitmap_encodes += 1;
        self.counters.bitmap_decodes += 1;
        self.counters.stable_inserts += u64::from(!removed);
        self.counters.stable_removes += u64::from(removed);
        self.mark_dirty(component);
        Ok(!updated.is_empty())
    }

    pub fn append_file(
        &mut self,
        file: FileEntry,
        collection: CollectionId,
        records: Vec<RecordEntry>,
    ) -> Result<Vec<RecordSlot>, StableIndexError> {
        let files_component = self
            .file_catalog_component()
            .ok_or_else(|| StableIndexError::MissingDependency("physical-file catalog".into()))?;
        let paths_component = self
            .path_catalog_component()
            .ok_or_else(|| StableIndexError::MissingDependency("path lookup catalog".into()))?;
        if file.id.0 != self.next_file_id()?.0 {
            return Err(StableIndexError::Format(format!(
                "file ID {} is not the next append-only identity {}",
                file.id.0,
                self.next_file_id()?.0
            )));
        }
        if self.file_catalog_record(file.id)?.is_some() {
            return Err(StableIndexError::Format("file ID already exists".into()));
        }
        let collection_info = self
            .collections
            .get(&collection)
            .ok_or_else(|| StableIndexError::Format("unknown collection".into()))?
            .clone();
        let base = collection_info.slot_high_water;
        let count = u32::try_from(records.len())
            .map_err(|_| StableIndexError::Format("too many appended records".into()))?;
        let high_water = base
            .checked_add(count)
            .ok_or_else(|| StableIndexError::Format("record slot overflow".into()))?;
        let path_key =
            super::PathKey::from_path(&file.relative_path).map_err(StableIndexError::Format)?;
        let mut path_refs = self.records_for_path_key(&path_key)?;
        let mut assigned = Vec::with_capacity(records.len());
        let mut encoded_records = Vec::with_capacity(records.len());
        for (offset, mut record) in records.into_iter().enumerate() {
            let offset = u32::try_from(offset)
                .map_err(|_| StableIndexError::Format("slot offset overflow".into()))?;
            let slot = RecordSlot(
                base.checked_add(offset)
                    .ok_or_else(|| StableIndexError::Format("record slot overflow".into()))?,
            );
            if record.file != file.id {
                return Err(StableIndexError::Format(
                    "record file ownership does not match the appended file".into(),
                ));
            }
            record.slot = slot;
            assigned.push(slot);
            path_refs.push(RecordRef { collection, slot });
            let value = codec::encode_meta(&RecordCatalogRecord {
                entry: record.clone(),
                live: true,
                incarnation: 1,
            })?;
            encoded_records.push((record, value));
        }
        path_refs.sort_unstable();
        path_refs.dedup();
        let path_value = codec::encode_meta(&PathCatalogEntry {
            file: file.id,
            refs: path_refs,
        })?;
        let file_value = codec::encode_meta(&super::codec::FileCatalogRecord {
            entry: file.clone(),
            live: true,
            incarnation: 1,
        })?;
        {
            let tree = self.tree_mut(files_component)?;
            tree.insert(ComponentKey::FileId(file.id.0), file_value);
        }
        {
            let tree = self.tree_mut(paths_component)?;
            tree.insert(ComponentKey::Path(path_key), path_value);
        }
        {
            let records_component = collection_info.records;
            let tree = self.tree_mut(records_component)?;
            for (record, value) in &encoded_records {
                tree.insert(ComponentKey::RecordSlot(record.slot.0), value.clone());
            }
        }
        for (record, _) in &encoded_records {
            self.add_identity_slot(collection, &record.id, record.slot)?;
            self.update_domain(collection, DOMAIN_ALL, Some(record.slot), None)?;
            self.update_domain(collection, DOMAIN_LIVE, Some(record.slot), None)?;
            match record.class {
                RecordClass::Logical => {
                    self.update_domain(collection, DOMAIN_LOGICAL, Some(record.slot), None)?;
                }
                RecordClass::RawFallback => {
                    self.update_domain(collection, DOMAIN_RAW_FALLBACK, Some(record.slot), None)?;
                }
            }
        }
        if let Some(info) = self.collections.get_mut(&collection) {
            info.slot_high_water = high_water;
            info.live_records = info.live_records.saturating_add(count as u64);
            info.generation = info.generation.saturating_add(1);
            info.invalidate_live_cache();
        }
        self.mark_dirty(files_component);
        self.mark_dirty(paths_component);
        self.mark_dirty(collection_info.records);
        self.mark_dirty(collection_info.identities);
        self.mark_dirty(collection_info.domains);
        self.counters.bitmap_encodes += 2;
        Ok(assigned)
    }

    pub fn delete_file_by_path(
        &mut self,
        path: &std::path::Path,
    ) -> Result<Vec<RecordSlot>, StableIndexError> {
        let key = super::PathKey::from_path(path).map_err(StableIndexError::Format)?;
        let refs = self.records_for_path_key(&key)?;
        if refs.is_empty() {
            return Ok(Vec::new());
        }
        let path_component = self
            .path_catalog_component()
            .ok_or_else(|| StableIndexError::MissingDependency("path lookup catalog".into()))?;
        let files_component = self
            .file_catalog_component()
            .ok_or_else(|| StableIndexError::MissingDependency("physical-file catalog".into()))?;
        let mut by_collection = BTreeMap::<CollectionId, Vec<RecordRef>>::new();
        for reference in refs {
            by_collection
                .entry(reference.collection)
                .or_default()
                .push(reference);
        }
        let mut deleted = Vec::new();
        let mut file_id = None;
        for (collection, references) in by_collection {
            let info = self
                .collections
                .get(&collection)
                .ok_or_else(|| StableIndexError::Format("unknown collection".into()))?
                .clone();
            for reference in references {
                let Some(catalog) = self.record_catalog_entry(collection, reference.slot)? else {
                    continue;
                };
                file_id = Some(catalog.entry.file);
                if !catalog.live {
                    continue;
                }
                let record_id = catalog.entry.id.clone();
                let class = catalog.entry.class;
                let tombstone = codec::encode_meta(&RecordCatalogRecord {
                    entry: catalog.entry,
                    live: false,
                    incarnation: catalog.incarnation.saturating_add(1),
                })?;
                {
                    let tree = self.tree_mut(info.records)?;
                    tree.insert(ComponentKey::RecordSlot(reference.slot.0), tombstone);
                }
                self.remove_identity_slot(collection, &record_id, reference.slot)?;
                self.update_domain(collection, DOMAIN_LIVE, None, Some(reference.slot))?;
                self.update_domain(collection, DOMAIN_ALL, None, Some(reference.slot))?;
                match class {
                    RecordClass::Logical => {
                        self.update_domain(collection, DOMAIN_LOGICAL, None, Some(reference.slot))?;
                    }
                    RecordClass::RawFallback => {
                        self.update_domain(
                            collection,
                            DOMAIN_RAW_FALLBACK,
                            None,
                            Some(reference.slot),
                        )?;
                    }
                }
                deleted.push(reference.slot);
            }
            let deleted_here = deleted.len() as u64;
            self.mark_dirty(info.records);
            self.mark_dirty(info.identities);
            self.mark_dirty(info.domains);
            self.invalidate_collection_cache(collection);
            if let Some(current) = self.collections.get_mut(&collection) {
                current.live_records = current.live_records.saturating_sub(deleted_here);
            }
        }
        {
            let tree = self.tree_mut(path_component)?;
            tree.remove(&ComponentKey::Path(key));
        }
        self.mark_dirty(path_component);
        if let Some(file_id) = file_id {
            if let Some(mut file) = self.file_catalog_record(file_id)? {
                file.live = false;
                file.incarnation = file.incarnation.saturating_add(1);
                let value = codec::encode_meta(&file)?;
                let tree = self.tree_mut(files_component)?;
                tree.insert(ComponentKey::FileId(file_id.0), value);
                self.mark_dirty(files_component);
            }
        }
        Ok(deleted)
    }

    pub fn replace_file(
        &mut self,
        old_path: &std::path::Path,
        mut new_file: FileEntry,
        collection: CollectionId,
        records: Vec<RecordEntry>,
    ) -> Result<Vec<RecordSlot>, StableIndexError> {
        let new_file_id = self.next_file_id()?;
        new_file.id = new_file_id;
        let mut prepared = Vec::with_capacity(records.len());
        for record in records {
            let mut record = record;
            record.file = new_file_id;
            prepared.push(record);
        }
        let deleted = self.delete_file_by_path(old_path)?;
        let result = self.append_file(new_file, collection, prepared);
        if result.is_err() {
            self.mark_prototype_invalid("file replacement failed after the old file was retired");
        }
        let _ = deleted;
        result
    }

    pub fn move_file(
        &mut self,
        file: FileId,
        new_relative_path: &std::path::Path,
    ) -> Result<(), StableIndexError> {
        let Some(catalog) = self.file_catalog_record(file)? else {
            return Err(StableIndexError::Format("unknown file".into()));
        };
        for collection in self.collections.values() {
            if !collection.identity_policy.contains("Generated") {
                return Err(StableIndexError::Unsupported(format!(
                    "moving a file whose identity policy is {} requires replacement",
                    collection.identity_policy
                )));
            }
        }
        let paths_component = self
            .path_catalog_component()
            .ok_or_else(|| StableIndexError::MissingDependency("path lookup catalog".into()))?;
        let files_component = self
            .file_catalog_component()
            .ok_or_else(|| StableIndexError::MissingDependency("physical-file catalog".into()))?;
        let old_key = super::PathKey::from_path(&catalog.entry.relative_path)
            .map_err(StableIndexError::Format)?;
        let new_key =
            super::PathKey::from_path(new_relative_path).map_err(StableIndexError::Format)?;
        let refs = self.records_for_path_key(&old_key)?;
        if !self.records_for_path_key(&new_key)?.is_empty() {
            return Err(StableIndexError::Format(
                "destination path already has records".into(),
            ));
        }
        {
            let tree = self.tree_mut(paths_component)?;
            tree.remove(&ComponentKey::Path(old_key));
            tree.insert(
                ComponentKey::Path(new_key),
                codec::encode_meta(&PathCatalogEntry { file, refs })?,
            );
        }
        let mut entry = catalog.entry;
        entry.relative_path = new_relative_path.to_path_buf().into();
        let value = codec::encode_meta(&super::codec::FileCatalogRecord {
            entry,
            live: catalog.live,
            incarnation: catalog.incarnation.saturating_add(1),
        })?;
        {
            let tree = self.tree_mut(files_component)?;
            tree.insert(ComponentKey::FileId(file.0), value);
        }
        self.mark_dirty(paths_component);
        self.mark_dirty(files_component);
        Ok(())
    }

    fn add_identity_slot(
        &mut self,
        collection: CollectionId,
        id: &RecordId,
        slot: RecordSlot,
    ) -> Result<(), StableIndexError> {
        let component = self
            .collections
            .get(&collection)
            .ok_or_else(|| StableIndexError::Format("unknown collection".into()))?
            .identities;
        let key = codec::identity_key(id);
        let tree = self.tree_mut(component)?;
        let mut bitmap = tree
            .get(&key)
            .map(|value| memory::decode_bitmap(&value))
            .transpose()
            .map_err(StableIndexError::Format)?
            .unwrap_or_default();
        bitmap.insert(slot.0);
        tree.insert(
            key,
            memory::bitmap_value(&bitmap).map_err(StableIndexError::Format)?,
        );
        self.counters.bitmap_encodes += 1;
        self.mark_dirty(component);
        Ok(())
    }

    fn remove_identity_slot(
        &mut self,
        collection: CollectionId,
        id: &RecordId,
        slot: RecordSlot,
    ) -> Result<(), StableIndexError> {
        let component = self
            .collections
            .get(&collection)
            .ok_or_else(|| StableIndexError::Format("unknown collection".into()))?
            .identities;
        let key = codec::identity_key(id);
        let tree = self.tree_mut(component)?;
        let Some(value) = tree.get(&key) else {
            return Ok(());
        };
        let mut bitmap = memory::decode_bitmap(&value).map_err(StableIndexError::Format)?;
        bitmap.remove(slot.0);
        if bitmap.is_empty() {
            tree.remove(&key);
        } else {
            tree.insert(
                key,
                memory::bitmap_value(&bitmap).map_err(StableIndexError::Format)?,
            );
        }
        self.counters.bitmap_encodes += 1;
        self.mark_dirty(component);
        Ok(())
    }

    fn update_domain(
        &mut self,
        collection: CollectionId,
        domain: u8,
        add: Option<RecordSlot>,
        remove: Option<RecordSlot>,
    ) -> Result<(), StableIndexError> {
        let component = self
            .collections
            .get(&collection)
            .ok_or_else(|| StableIndexError::Format("unknown collection".into()))?
            .domains;
        let key = ComponentKey::Domain(domain);
        let tree = self.tree_mut(component)?;
        let mut bitmap = tree
            .get(&key)
            .map(|value| memory::decode_bitmap(&value))
            .transpose()
            .map_err(StableIndexError::Format)?
            .unwrap_or_default();
        if let Some(slot) = add {
            bitmap.insert(slot.0);
        }
        if let Some(slot) = remove {
            bitmap.remove(slot.0);
        }
        tree.insert(
            key,
            memory::bitmap_value(&bitmap).map_err(StableIndexError::Format)?,
        );
        self.counters.bitmap_encodes += 1;
        self.mark_dirty(component);
        self.invalidate_collection_cache(collection);
        Ok(())
    }
}

impl StandaloneIndex {
    pub fn raw_update_posting(
        &mut self,
        key: &ComponentKey,
        add: &RoaringBitmap,
        remove: &RoaringBitmap,
    ) -> Result<RoaringBitmap, StableIndexError> {
        let current = self
            .raw_get(key)?
            .map(|value| memory::decode_bitmap(&value))
            .transpose()
            .map_err(StableIndexError::Format)?
            .unwrap_or_default();
        let mut updated = current;
        updated |= add;
        updated -= remove;
        if updated.is_empty() {
            self.raw_remove(key)?;
        } else {
            self.raw_insert(
                key.clone(),
                memory::bitmap_value(&updated).map_err(StableIndexError::Format)?,
            )?;
        }
        Ok(updated)
    }

    pub fn raw_set(
        &mut self,
        key: ComponentKey,
        value: StoredBytes,
    ) -> Result<(), StableIndexError> {
        self.raw_insert(key, value)?;
        Ok(())
    }
}
