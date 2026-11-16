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
        if let Some(next) = self.next_file_id {
            return Ok(FileId(next));
        }
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
        self.next_file_id = Some(next);
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
        self.touch_component(component);
        Ok(!updated.is_empty())
    }

    pub fn append_prepared_file(
        &mut self,
        change: super::PreparedFileChange,
    ) -> Result<Vec<RecordSlot>, StableIndexError> {
        self.apply_prepared_file(change, None)
    }

    pub fn replace_prepared_file(
        &mut self,
        old_path: &std::path::Path,
        mut change: super::PreparedFileChange,
    ) -> Result<Vec<RecordSlot>, StableIndexError> {
        let new_file_id = self.next_file_id()?;
        change.file.id = new_file_id;
        for collection in &mut change.collections {
            for record in &mut collection.records {
                record.file = new_file_id;
            }
        }
        // Preparation is complete before the old file is retired.
        let result = self.apply_prepared_file(change, Some(old_path));
        if result.is_err() {
            self.mark_prototype_invalid("prepared file replacement failed after mutation began");
        }
        result
    }

    #[allow(dead_code)]
    fn append_file(
        &mut self,
        file: FileEntry,
        collection: CollectionId,
        records: Vec<RecordEntry>,
    ) -> Result<Vec<RecordSlot>, StableIndexError> {
        self.append_prepared_file(super::PreparedFileChange {
            file,
            collections: vec![super::PreparedCollectionChange {
                collection,
                records,
                posting_adds: Vec::new(),
                coverage: Vec::new(),
            }],
        })
    }

    fn apply_prepared_file(
        &mut self,
        change: super::PreparedFileChange,
        replace_old_path: Option<&std::path::Path>,
    ) -> Result<Vec<RecordSlot>, StableIndexError> {
        self.require_usable()?;
        let file = change.file;
        let files_component = self
            .file_catalog_component()
            .ok_or_else(|| StableIndexError::MissingDependency("physical-file catalog".into()))?;
        let paths_component = self
            .path_catalog_component()
            .ok_or_else(|| StableIndexError::MissingDependency("path lookup catalog".into()))?;
        let expected = self.next_file_id()?.0;
        if file.id.0 != expected {
            return Err(StableIndexError::Format(format!(
                "file ID {} is not the next append-only identity {expected}",
                file.id.0
            )));
        }
        if self.file_catalog_record(file.id)?.is_some() {
            return Err(StableIndexError::Format("file ID already exists".into()));
        }
        let path_key =
            super::PathKey::from_path(&file.relative_path).map_err(StableIndexError::Format)?;
        if replace_old_path.is_none() {
            if let Some(existing) = self.path_entry(&path_key)? {
                if existing.file != file.id {
                    return Err(StableIndexError::Format(
                        "destination path is already occupied".into(),
                    ));
                }
            }
        }
        let mut runtime = Vec::with_capacity(change.collections.len());
        let mut path_refs = Vec::new();
        let mut all_assigned = Vec::new();
        for collection_change in change.collections {
            let info = self
                .collections
                .get(&collection_change.collection)
                .ok_or_else(|| StableIndexError::Format("unknown collection".into()))?
                .clone();
            let base = info.slot_high_water;
            let count = u32::try_from(collection_change.records.len())
                .map_err(|_| StableIndexError::Format("too many appended records".into()))?;
            let high_water = base
                .checked_add(count)
                .ok_or_else(|| StableIndexError::Format("record slot overflow".into()))?;
            let mut encoded_records = Vec::with_capacity(collection_change.records.len());
            let mut identities = BTreeMap::<RecordId, RoaringBitmap>::new();
            let mut logical_add = RoaringBitmap::new();
            let mut raw_add = RoaringBitmap::new();
            let mut assigned = Vec::new();
            for (offset, mut record) in collection_change.records.into_iter().enumerate() {
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
                all_assigned.push(slot);
                path_refs.push(RecordRef {
                    collection: collection_change.collection,
                    slot,
                });
                match record.class {
                    RecordClass::Logical => {
                        logical_add.insert(slot.0);
                    }
                    RecordClass::RawFallback => {
                        raw_add.insert(slot.0);
                    }
                }
                identities
                    .entry(record.id.clone())
                    .or_default()
                    .insert(slot.0);
                let value = codec::encode_meta(&RecordCatalogRecord {
                    entry: record.clone(),
                    live: true,
                    incarnation: 1,
                })?;
                encoded_records.push((record, value));
            }
            for add in &collection_change.posting_adds {
                if add.slots.max().is_some_and(|slot| slot >= count) {
                    return Err(StableIndexError::Format(
                        "prepared posting slot is outside the appended record set".into(),
                    ));
                }
            }
            runtime.push((
                collection_change.collection,
                info,
                base,
                count,
                high_water,
                encoded_records,
                identities,
                logical_add,
                raw_add,
                collection_change.posting_adds,
                collection_change.coverage,
                assigned,
            ));
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
        if let Some(old_path) = replace_old_path {
            self.delete_file_by_path(old_path)?;
        }
        {
            let tree = self.tree_mut(files_component)?;
            tree.insert(ComponentKey::FileId(file.id.0), file_value);
        }
        super::snapshot::inject_fault(
            "mutation-after-first-change",
            Some(file.relative_path.as_path()),
        )
        .map_err(StableIndexError::Io)?;
        {
            let tree = self.tree_mut(paths_component)?;
            tree.insert(ComponentKey::Path(path_key), path_value);
        }
        for (
            collection,
            info,
            base,
            count,
            high_water,
            encoded_records,
            identities,
            logical_add,
            raw_add,
            posting_adds,
            coverage,
            assigned,
        ) in runtime
        {
            {
                let tree = self.tree_mut(info.records)?;
                for (record, value) in &encoded_records {
                    tree.insert(ComponentKey::RecordSlot(record.slot.0), value.clone());
                }
            }
            for (id, slots) in identities {
                self.add_identity_slots(collection, &id, &slots)?;
            }
            let all_add = RoaringBitmap::from_iter(assigned.iter().map(|slot| slot.0));
            self.update_domain_batch(collection, DOMAIN_ALL, &all_add, &RoaringBitmap::new())?;
            self.update_domain_batch(collection, DOMAIN_LIVE, &all_add, &RoaringBitmap::new())?;
            self.update_domain_batch(
                collection,
                DOMAIN_LOGICAL,
                &logical_add,
                &RoaringBitmap::new(),
            )?;
            self.update_domain_batch(
                collection,
                DOMAIN_RAW_FALLBACK,
                &raw_add,
                &RoaringBitmap::new(),
            )?;
            for add in posting_adds {
                let key = shift_component_key(add.key, base)?;
                if let Some(value) = add.value {
                    let tree = self.tree_mut(add.component)?;
                    tree.insert(key, value);
                    self.touch_component(add.component);
                } else {
                    let slots = RoaringBitmap::from_iter(add.slots.iter().map(|slot| slot + base));
                    self.update_posting(
                        collection,
                        add.component,
                        key,
                        &slots,
                        &RoaringBitmap::new(),
                    )?;
                }
            }
            for (component, coverage) in coverage {
                if let Some(entry) = self.components.get_mut(&component) {
                    entry.descriptor.coverage = coverage;
                }
                self.touch_component(component);
            }
            if let Some(current) = self.collections.get_mut(&collection) {
                current.slot_high_water = high_water;
                current.live_records = current
                    .live_records
                    .checked_add(count as u64)
                    .ok_or_else(|| StableIndexError::InvalidState("live record overflow".into()))?;
                current.generation = current.generation.saturating_add(1);
                current.invalidate_live_cache();
            }
            self.mark_dirty(info.records);
            self.mark_dirty(info.identities);
            self.mark_dirty(info.domains);
        }
        self.next_file_id = Some(file.id.0.saturating_add(1));
        self.mark_dirty(files_component);
        self.mark_dirty(paths_component);
        Ok(all_assigned)
    }

    pub fn delete_file_by_path(
        &mut self,
        path: &std::path::Path,
    ) -> Result<Vec<RecordRef>, StableIndexError> {
        let key = super::PathKey::from_path(path).map_err(StableIndexError::Format)?;
        let path_component = self
            .path_catalog_component()
            .ok_or_else(|| StableIndexError::MissingDependency("path lookup catalog".into()))?;
        let files_component = self
            .file_catalog_component()
            .ok_or_else(|| StableIndexError::MissingDependency("physical-file catalog".into()))?;
        let Some(entry) = self.path_entry(&key)? else {
            return Ok(Vec::new());
        };
        let file_id = entry.file;
        let mut by_collection = BTreeMap::<CollectionId, Vec<RecordRef>>::new();
        for reference in entry.refs {
            by_collection
                .entry(reference.collection)
                .or_default()
                .push(reference);
        }
        let mut deleted = Vec::new();
        for (collection, references) in by_collection {
            let info = self
                .collections
                .get(&collection)
                .ok_or_else(|| StableIndexError::Format("unknown collection".into()))?
                .clone();
            let mut live_remove = RoaringBitmap::new();
            let mut logical_remove = RoaringBitmap::new();
            let mut raw_remove = RoaringBitmap::new();
            let mut identities = BTreeMap::<RecordId, RoaringBitmap>::new();
            let mut retired_here = 0u64;
            for reference in references {
                let Some(catalog) = self.record_catalog_entry(collection, reference.slot)? else {
                    continue;
                };
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
                identities
                    .entry(record_id)
                    .or_default()
                    .insert(reference.slot.0);
                live_remove.insert(reference.slot.0);
                match class {
                    RecordClass::Logical => {
                        logical_remove.insert(reference.slot.0);
                    }
                    RecordClass::RawFallback => {
                        raw_remove.insert(reference.slot.0);
                    }
                }
                retired_here = retired_here.checked_add(1).ok_or_else(|| {
                    StableIndexError::InvalidState("delete count overflow".into())
                })?;
                deleted.push(reference);
            }
            for (id, slots) in identities {
                self.remove_identity_slots(collection, &id, &slots)?;
            }
            self.update_domain_batch(collection, DOMAIN_LIVE, &RoaringBitmap::new(), &live_remove)?;
            self.update_domain_batch(collection, DOMAIN_ALL, &RoaringBitmap::new(), &live_remove)?;
            self.update_domain_batch(
                collection,
                DOMAIN_LOGICAL,
                &RoaringBitmap::new(),
                &logical_remove,
            )?;
            self.update_domain_batch(
                collection,
                DOMAIN_RAW_FALLBACK,
                &RoaringBitmap::new(),
                &raw_remove,
            )?;
            self.mark_dirty(info.records);
            self.mark_dirty(info.identities);
            self.mark_dirty(info.domains);
            self.invalidate_collection_cache(collection);
            if let Some(current) = self.collections.get_mut(&collection) {
                current.live_records =
                    current
                        .live_records
                        .checked_sub(retired_here)
                        .ok_or_else(|| {
                            StableIndexError::InvalidState(
                                "collection live count is smaller than the retired set".into(),
                            )
                        })?;
            }
        }
        {
            let tree = self.tree_mut(path_component)?;
            tree.remove(&ComponentKey::Path(key));
        }
        self.mark_dirty(path_component);
        if let Some(mut file) = self.file_catalog_record(file_id)? {
            file.live = false;
            file.incarnation = file.incarnation.saturating_add(1);
            let value = codec::encode_meta(&file)?;
            let tree = self.tree_mut(files_component)?;
            tree.insert(ComponentKey::FileId(file_id.0), value);
            self.mark_dirty(files_component);
        }
        Ok(deleted)
    }

    #[allow(dead_code)]
    pub(crate) fn replace_file(
        &mut self,
        old_path: &std::path::Path,
        mut new_file: FileEntry,
        collection: CollectionId,
        records: Vec<RecordEntry>,
    ) -> Result<Vec<RecordSlot>, StableIndexError> {
        let new_file_id = self.next_file_id()?;
        new_file.id = new_file_id;
        let mut prepared = Vec::with_capacity(records.len());
        for mut record in records {
            record.file = new_file_id;
            prepared.push(record);
        }
        let change = super::PreparedFileChange {
            file: new_file,
            collections: vec![super::PreparedCollectionChange {
                collection,
                records: prepared,
                posting_adds: Vec::new(),
                coverage: Vec::new(),
            }],
        };
        self.replace_prepared_file(old_path, change)
    }

    pub fn move_file(
        &mut self,
        file: FileId,
        new_relative_path: &std::path::Path,
    ) -> Result<(), StableIndexError> {
        let Some(catalog) = self.file_catalog_record(file)? else {
            return Err(StableIndexError::Format("unknown file".into()));
        };
        if !catalog.live {
            return Err(StableIndexError::Format("file is not live".into()));
        }
        if catalog.entry.relative_path.as_path() == new_relative_path {
            return Ok(());
        }
        for collection_id in &catalog.entry.collections {
            let policy = self
                .collections
                .get(collection_id)
                .ok_or_else(|| StableIndexError::Format("unknown file collection".into()))?
                .identity_policy
                .clone();
            let allowed = match policy {
                super::PersistedIdentityPolicy::Generated => true,
                super::PersistedIdentityPolicy::Filename => {
                    catalog.entry.relative_path.file_name() == new_relative_path.file_name()
                }
                super::PersistedIdentityPolicy::RelativePath
                | super::PersistedIdentityPolicy::Intrinsic(_) => false,
            };
            if !allowed {
                return Err(StableIndexError::Unsupported(format!(
                    "moving a file with identity policy {:?} requires replacement",
                    policy
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
        let source = self
            .path_entry(&old_key)?
            .ok_or_else(|| StableIndexError::Format("source path mapping is absent".into()))?;
        if self.path_entry(&new_key)?.is_some() {
            return Err(StableIndexError::Format(
                "destination path already has a mapping".into(),
            ));
        }
        {
            let tree = self.tree_mut(paths_component)?;
            tree.remove(&ComponentKey::Path(old_key));
            tree.insert(
                ComponentKey::Path(new_key),
                codec::encode_meta(&PathCatalogEntry {
                    file,
                    refs: source.refs,
                })?,
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

    fn add_identity_slots(
        &mut self,
        collection: CollectionId,
        id: &RecordId,
        slots: &RoaringBitmap,
    ) -> Result<(), StableIndexError> {
        if slots.is_empty() {
            return Ok(());
        }
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
        bitmap |= slots;
        tree.insert(
            key,
            memory::bitmap_value(&bitmap).map_err(StableIndexError::Format)?,
        );
        self.counters.bitmap_decodes += 1;
        self.counters.bitmap_encodes += 1;
        self.touch_component(component);
        Ok(())
    }

    fn remove_identity_slots(
        &mut self,
        collection: CollectionId,
        id: &RecordId,
        slots: &RoaringBitmap,
    ) -> Result<(), StableIndexError> {
        if slots.is_empty() {
            return Ok(());
        }
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
        bitmap -= slots;
        if bitmap.is_empty() {
            tree.remove(&key);
        } else {
            tree.insert(
                key,
                memory::bitmap_value(&bitmap).map_err(StableIndexError::Format)?,
            );
        }
        self.counters.bitmap_decodes += 1;
        self.counters.bitmap_encodes += 1;
        self.touch_component(component);
        Ok(())
    }

    fn update_domain_batch(
        &mut self,
        collection: CollectionId,
        domain: u8,
        add: &RoaringBitmap,
        remove: &RoaringBitmap,
    ) -> Result<(), StableIndexError> {
        if add.is_empty() && remove.is_empty() {
            return Ok(());
        }
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
        bitmap |= add;
        bitmap -= remove;
        tree.insert(
            key,
            memory::bitmap_value(&bitmap).map_err(StableIndexError::Format)?,
        );
        self.counters.bitmap_decodes += 1;
        self.counters.bitmap_encodes += 1;
        self.touch_component(component);
        self.invalidate_collection_cache(collection);
        Ok(())
    }
}

fn shift_component_key(key: ComponentKey, base: u32) -> Result<ComponentKey, StableIndexError> {
    Ok(match key {
        ComponentKey::RecordSlot(slot) => ComponentKey::RecordSlot(
            slot.checked_add(base)
                .ok_or_else(|| StableIndexError::Format("record slot overflow".into()))?,
        ),
        ComponentKey::Sort(mut key) => {
            key.slot.0 = key
                .slot
                .0
                .checked_add(base)
                .ok_or_else(|| StableIndexError::Format("sort slot overflow".into()))?;
            ComponentKey::Sort(key)
        }
        ComponentKey::Occurrence(mut key) => {
            key.source.0 = key
                .source
                .0
                .checked_add(base)
                .ok_or_else(|| StableIndexError::Format("relation source overflow".into()))?;
            ComponentKey::Occurrence(key)
        }
        other => other,
    })
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
