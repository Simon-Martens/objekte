//! Experimental independent stable-index structures.
//!
//! This module is feature gated by `ic-stable-prototype`.  It is deliberately
//! separate from the production repository: every component owns one
//! `RefCell<Vec<u8>>` image, can be saved as a standalone file, and is loaded
//! with `BTreeMap::load` rather than by replaying insertions.

mod build;
mod catalog;
mod codec;
mod memory;
mod mutation;
mod posting;
mod snapshot;
mod validation;

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use roaring::RoaringBitmap;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::model::CollectionId;
use crate::repository::Repository;

pub use codec::{
    BindingRole, CollectionIdentity, ComponentDescriptor, ComponentId, ComponentKey, ComponentKind,
    CoverageDescriptor, DOMAIN_ALL, DOMAIN_EMPTY_ARRAY, DOMAIN_LIVE, DOMAIN_LOGICAL,
    DOMAIN_RAW_FALLBACK, FieldIdentity, OrderingDescriptor, PathCatalogEntry, PathKey, RecordRef,
    RelationOccurrenceValue, STATE_ANY_NULL, STATE_ARRAY, STATE_EMPTY_OBJECT,
    STATE_EMPTY_SELECTION, STATE_LEGACY_NULL, STATE_NON_EMPTY, STATE_NUMBER, STATE_OBJECT,
    STATE_PRESENCE, STATE_STRING, SortEntryKey,
};
pub use memory::{IndexMemory, IndexTree, StoredBytes};
pub use snapshot::{
    ManifestCollection, ManifestComponent, SnapshotManifest, component_relative_path, read_manifest,
};
pub use validation::ValidationReport;

const TREE_HEADER_BYTES: usize = codec::TREE_HEADER_BYTES;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct FieldComponentKey {
    pub path: Vec<String>,
    pub binding: Option<String>,
    pub role: BindingRole,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct TextComponentKey {
    pub path: Vec<String>,
    pub binding: Option<String>,
    pub role: BindingRole,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct SortComponentKey(pub String);

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct RelationComponentKey {
    pub path: Vec<String>,
    pub target_collection: String,
}

#[derive(Clone, Debug, Default)]
pub struct FieldComponentIds {
    pub values: Option<ComponentId>,
    pub lengths: Option<ComponentId>,
    pub states: Option<ComponentId>,
}

#[derive(Clone, Debug, Default)]
pub struct TextComponentIds {
    pub normal: Option<ComponentId>,
    pub skip: Option<ComponentId>,
}

#[derive(Clone, Debug)]
pub struct SortComponentIds {
    pub ascending: ComponentId,
    pub descending: ComponentId,
    pub options: OrderingDescriptor,
}

#[derive(Clone, Debug)]
pub struct RelationComponentIds {
    pub occurrences: ComponentId,
    pub targets: ComponentId,
    pub states: ComponentId,
    pub declaration: crate::relation::RelationDefinition,
    pub target_collection: CollectionId,
}

#[derive(Clone, Debug)]
pub struct StableCollection {
    pub id: CollectionId,
    pub name: String,
    pub generation: u64,
    pub slot_high_water: u32,
    pub live_records: u64,
    pub identity_policy: String,
    pub records: ComponentId,
    pub identities: ComponentId,
    pub domains: ComponentId,
    pub fields: BTreeMap<FieldComponentKey, FieldComponentIds>,
    pub texts: BTreeMap<TextComponentKey, TextComponentIds>,
    pub sorts: BTreeMap<SortComponentKey, SortComponentIds>,
    pub relations: BTreeMap<RelationComponentKey, RelationComponentIds>,
    live_cache: Option<RoaringBitmap>,
}

impl StableCollection {
    fn invalidate_live_cache(&mut self) {
        self.live_cache = None;
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct PrototypeCounters {
    pub component_loads: u64,
    pub component_payload_bytes_read: u64,
    pub manifest_opens: u64,
    pub standalone_files_opened: u64,
    pub stable_gets: u64,
    pub stable_inserts: u64,
    pub stable_removes: u64,
    pub bitmap_encodes: u64,
    pub bitmap_decodes: u64,
    pub component_inserts_during_load: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct ComponentBuildRecord {
    pub id: ComponentId,
    pub kind: ComponentKind,
    pub collection: Option<CollectionId>,
    pub field: Option<String>,
    pub entry_count: u64,
    pub payload_len: u64,
    pub duration_ns: u64,
    pub start_offset_ns: u64,
    pub end_offset_ns: u64,
    pub worker: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ComponentStatistics {
    pub id: ComponentId,
    pub kind: ComponentKind,
    pub state: String,
    pub entry_count: u64,
    pub payload_len: Option<u64>,
    pub descriptor: ComponentDescriptor,
    pub dirty: bool,
    pub path: Option<PathBuf>,
}

pub(crate) enum ComponentState {
    Loaded {
        tree: IndexTree,
        dirty: bool,
    },
    Unloaded {
        path: PathBuf,
        info: codec::ComponentFileInfo,
    },
    #[allow(dead_code)]
    Failed(String),
}

impl ComponentState {
    fn label(&self) -> String {
        match self {
            Self::Loaded { dirty, .. } if *dirty => "loaded-dirty".into(),
            Self::Loaded { .. } => "loaded".into(),
            Self::Unloaded { .. } => "unloaded".into(),
            Self::Failed(error) => format!("failed: {error}"),
        }
    }
}

pub(crate) struct ComponentEntry {
    pub descriptor: ComponentDescriptor,
    pub state: ComponentState,
    pub saved_info: Option<codec::ComponentFileInfo>,
}

impl ComponentEntry {
    pub fn entry_count(&self) -> u64 {
        match &self.state {
            ComponentState::Loaded { tree, .. } => tree.len(),
            ComponentState::Unloaded { info, .. } => info.envelope.entry_count,
            ComponentState::Failed(_) => 0,
        }
    }

    pub fn dirty(&self) -> bool {
        matches!(&self.state, ComponentState::Loaded { dirty: true, .. })
    }

    pub fn path(&self) -> Option<PathBuf> {
        match &self.state {
            ComponentState::Loaded { .. } => self.saved_info.as_ref().map(|info| info.path.clone()),
            ComponentState::Unloaded { path, .. } => Some(path.clone()),
            ComponentState::Failed(_) => None,
        }
    }
}

#[derive(Debug)]
pub enum StableIndexError {
    Format(String),
    MissingComponent(ComponentId),
    MissingDependency(String),
    Unsupported(String),
    Io(String),
    InvalidState(String),
}

impl std::fmt::Display for StableIndexError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Format(message) => write!(formatter, "format error: {message}"),
            Self::MissingComponent(id) => write!(formatter, "component {} is not present", id.0),
            Self::MissingDependency(message) => write!(formatter, "missing dependency: {message}"),
            Self::Unsupported(message) => write!(formatter, "unsupported: {message}"),
            Self::Io(message) => write!(formatter, "I/O error: {message}"),
            Self::InvalidState(message) => write!(formatter, "invalid prototype state: {message}"),
        }
    }
}

impl std::error::Error for StableIndexError {}

impl From<String> for StableIndexError {
    fn from(value: String) -> Self {
        Self::Format(value)
    }
}

impl From<StableIndexError> for String {
    fn from(value: StableIndexError) -> Self {
        value.to_string()
    }
}

pub struct StablePrototype {
    pub(crate) repository_identity: [u8; 32],
    pub(crate) configuration_identity: [u8; 32],
    pub(crate) schema_fingerprint: Option<[u8; 32]>,
    pub(crate) slot_namespace: String,
    pub(crate) collections: BTreeMap<CollectionId, StableCollection>,
    pub(crate) components: BTreeMap<ComponentId, ComponentEntry>,
    pub(crate) generation: Option<PathBuf>,
    pub(crate) manifest: Option<SnapshotManifest>,
    pub(crate) counters: PrototypeCounters,
    pub(crate) usable: bool,
    pub(crate) build_records: Vec<ComponentBuildRecord>,
}

impl StablePrototype {
    pub fn from_repository(repository: &Repository) -> Result<Self, String> {
        build::from_repository(repository, None)
    }

    pub fn from_repository_with_workers(
        repository: &Repository,
        workers: usize,
    ) -> Result<Self, String> {
        build::from_repository(repository, Some(workers))
    }

    pub fn collections(&self) -> impl Iterator<Item = &StableCollection> {
        self.collections.values()
    }

    pub fn collection(&self, id: CollectionId) -> Option<&StableCollection> {
        self.collections.get(&id)
    }

    pub fn component_descriptors(&self) -> impl Iterator<Item = &ComponentDescriptor> {
        self.components.values().map(|entry| &entry.descriptor)
    }

    pub fn component_descriptor(&self, id: ComponentId) -> Option<&ComponentDescriptor> {
        self.components.get(&id).map(|entry| &entry.descriptor)
    }

    pub fn component_statistics(&self, id: ComponentId) -> Result<ComponentStatistics, String> {
        let entry = self
            .components
            .get(&id)
            .ok_or_else(|| format!("component {} is not present", id.0))?;
        Ok(ComponentStatistics {
            id,
            kind: entry.descriptor.kind,
            state: entry.state.label(),
            entry_count: entry.entry_count(),
            payload_len: entry
                .saved_info
                .as_ref()
                .map(|info| info.envelope.payload_len),
            descriptor: entry.descriptor.clone(),
            dirty: entry.dirty(),
            path: entry.path(),
        })
    }

    pub fn counters(&self) -> &PrototypeCounters {
        &self.counters
    }

    pub fn build_records(&self) -> &[ComponentBuildRecord] {
        &self.build_records
    }

    pub fn generation_path(&self) -> Option<&Path> {
        self.generation.as_deref()
    }

    pub fn slot_namespace(&self) -> &str {
        &self.slot_namespace
    }

    pub fn load_component(&mut self, id: ComponentId) -> Result<(), StableIndexError> {
        let entry = self
            .components
            .get_mut(&id)
            .ok_or(StableIndexError::MissingComponent(id))?;
        match &entry.state {
            ComponentState::Loaded { .. } => return Ok(()),
            ComponentState::Failed(error) => return Err(StableIndexError::Format(error.clone())),
            ComponentState::Unloaded { path, info } => {
                let path = path.clone();
                let info = info.clone();
                let file = codec::read_component_file(&path).map_err(StableIndexError::Format)?;
                if file.info.envelope != info.envelope {
                    return Err(StableIndexError::Format(
                        "component file changed after the manifest was opened".into(),
                    ));
                }
                let tree = load_tree_catching(file.payload)?;
                if tree.len() != info.envelope.entry_count {
                    return Err(StableIndexError::Format(format!(
                        "component {} entry count {} does not match the manifest count {}",
                        id.0,
                        tree.len(),
                        info.envelope.entry_count
                    )));
                }
                self.counters.component_loads += 1;
                self.counters.component_payload_bytes_read += info.envelope.payload_len;
                entry.state = ComponentState::Loaded { tree, dirty: false };
                entry.saved_info = Some(file.info);
                Ok(())
            }
        }
    }

    pub(crate) fn tree(&mut self, id: ComponentId) -> Result<&IndexTree, StableIndexError> {
        self.load_component(id)?;
        match &self.components.get(&id).expect("loaded component").state {
            ComponentState::Loaded { tree, .. } => Ok(tree),
            _ => unreachable!("load_component installs a loaded component"),
        }
    }

    pub(crate) fn tree_mut(&mut self, id: ComponentId) -> Result<&mut IndexTree, StableIndexError> {
        self.load_component(id)?;
        match &mut self
            .components
            .get_mut(&id)
            .expect("loaded component")
            .state
        {
            ComponentState::Loaded { tree, .. } => Ok(tree),
            _ => unreachable!("load_component installs a loaded component"),
        }
    }

    pub(crate) fn mark_dirty(&mut self, id: ComponentId) {
        if let Some(ComponentEntry {
            state: ComponentState::Loaded { dirty, .. },
            ..
        }) = self.components.get_mut(&id)
        {
            *dirty = true;
        }
    }

    pub(crate) fn mark_prototype_invalid(&mut self, error: impl Into<String>) {
        self.usable = false;
        let _ = error.into();
    }

    fn require_usable(&self) -> Result<(), StableIndexError> {
        if self.usable {
            Ok(())
        } else {
            Err(StableIndexError::InvalidState(
                "the prototype was left unusable by an earlier mutation failure".into(),
            ))
        }
    }

    pub fn save_snapshot(
        &mut self,
        new_generation_directory: impl AsRef<Path>,
    ) -> Result<PathBuf, StableIndexError> {
        self.require_usable()?;
        let destination = new_generation_directory.as_ref();
        if destination.exists() {
            return Err(StableIndexError::Io(format!(
                "destination {} already exists",
                destination.display()
            )));
        }
        let staging = snapshot::staging_directory(destination, self.components.len() as u64)
            .map_err(StableIndexError::Io)?;
        std::fs::create_dir(&staging).map_err(|error| {
            StableIndexError::Io(format!("create staging {}: {error}", staging.display()))
        })?;
        let result = self.write_snapshot_to(&staging);
        match result {
            Ok(manifest) => {
                snapshot::sync_directory(&staging).map_err(StableIndexError::Io)?;
                std::fs::rename(&staging, destination).map_err(|error| {
                    StableIndexError::Io(format!(
                        "publish generation {}: {error}",
                        destination.display()
                    ))
                })?;
                if let Some(parent) = destination.parent() {
                    snapshot::sync_directory(parent).map_err(StableIndexError::Io)?;
                }
                self.rebase_component_paths(destination);
                self.generation = Some(destination.to_path_buf());
                self.manifest = Some(manifest);
                Ok(destination.to_path_buf())
            }
            Err(error) => {
                let _ = std::fs::remove_dir_all(&staging);
                Err(error)
            }
        }
    }

    fn write_snapshot_to(
        &mut self,
        directory: &Path,
    ) -> Result<SnapshotManifest, StableIndexError> {
        let mut components = Vec::with_capacity(self.components.len());
        let ids = self.components.keys().copied().collect::<Vec<_>>();
        for id in ids {
            let entry = self.components.get_mut(&id).expect("component ID");
            let descriptor = entry.descriptor.clone();
            let relative = component_relative_path(&descriptor);
            let output = directory.join(&relative);
            if let Some(parent) = output.parent() {
                std::fs::create_dir_all(parent).map_err(|error| {
                    StableIndexError::Io(format!(
                        "create component directory {}: {error}",
                        parent.display()
                    ))
                })?;
            }
            let info = match &mut entry.state {
                ComponentState::Loaded { tree, dirty } => {
                    if *dirty {
                        let payload = std::mem::replace(tree, memory::new_tree());
                        let payload = memory::tree_payload(payload);
                        let write_result =
                            codec::write_component_file(&output, &descriptor, &payload, {
                                // The count is read from the tree header only
                                // after restoration; this placeholder is
                                // replaced below.
                                0
                            });
                        let restored = load_tree_catching(payload)?;
                        *tree = restored;
                        let mut info = write_result.map_err(StableIndexError::Io)?;
                        info.envelope.entry_count = tree.len();
                        *dirty = false;
                        // Rewriting the envelope with the exact count keeps
                        // the on-disk count and payload in one verified file.
                        rewrite_envelope_entry_count(&output, &mut info, tree.len())
                            .map_err(StableIndexError::Io)?;
                        info
                    } else {
                        // Use the same memory-image extraction path even for
                        // a clean component; this avoids a second clone.
                        let payload = std::mem::replace(tree, memory::new_tree());
                        let payload = memory::tree_payload(payload);
                        let entry_count = tree_header_entry_count(&payload);
                        let payload_len = payload.len();
                        let write_result = codec::write_component_file(
                            &output,
                            &descriptor,
                            &payload,
                            entry_count,
                        );
                        let restored = load_tree_catching(payload)?;
                        *tree = restored;
                        let info = write_result.map_err(StableIndexError::Io)?;
                        if info.envelope.payload_len != payload_len as u64 {
                            return Err(StableIndexError::Format(
                                "component payload length changed during save".into(),
                            ));
                        }
                        info
                    }
                }
                ComponentState::Unloaded { path, info } => {
                    if *path == output {
                        info.clone()
                    } else {
                        copy_component_file_verified(path, &output, info)
                            .map_err(StableIndexError::Io)?
                    }
                }
                ComponentState::Failed(error) => {
                    return Err(StableIndexError::InvalidState(format!(
                        "component {} failed and cannot be saved: {error}",
                        id.0
                    )));
                }
            };
            entry.saved_info = Some(info.clone());
            components.push(manifest_component(&descriptor, &relative, &info)?);
        }
        let collection_headers = self
            .collections
            .values()
            .map(|collection| {
                (
                    collection.id,
                    collection.name.clone(),
                    collection.generation,
                    collection.slot_high_water,
                )
            })
            .collect::<Vec<_>>();
        let mut collections = Vec::with_capacity(collection_headers.len());
        for (id, name, generation, slot_high_water) in collection_headers {
            collections.push(ManifestCollection {
                id: id.0,
                name,
                generation,
                slot_high_water,
                live_records: self
                    .collections
                    .get(&id)
                    .map_or(0, |collection| collection.live_records),
                identity_policy: self
                    .collections
                    .get(&id)
                    .map(|collection| collection.identity_policy.clone())
                    .unwrap_or_default(),
            });
        }
        let manifest = SnapshotManifest::new(
            self.repository_identity,
            self.configuration_identity,
            self.schema_fingerprint,
            self.slot_namespace.clone(),
            collections,
            components,
        );
        snapshot::write_manifest(directory, &manifest).map_err(StableIndexError::Io)?;
        Ok(manifest)
    }

    fn rebase_component_paths(&mut self, generation: &Path) {
        for entry in self.components.values_mut() {
            let relative = component_relative_path(&entry.descriptor);
            let path = generation.join(relative);
            if let Some(info) = entry.saved_info.as_mut() {
                info.path = path.clone();
            }
            if let ComponentState::Unloaded { path: existing, .. } = &mut entry.state {
                *existing = path;
            }
        }
    }

    pub fn save_index(
        &mut self,
        component_id: ComponentId,
        new_file_path: impl AsRef<Path>,
    ) -> Result<SaveIndexInfo, StableIndexError> {
        self.require_usable()?;
        let destination = new_file_path.as_ref();
        if destination.exists() {
            return Err(StableIndexError::Io(format!(
                "destination {} already exists",
                destination.display()
            )));
        }
        let entry = self
            .components
            .get_mut(&component_id)
            .ok_or(StableIndexError::MissingComponent(component_id))?;
        let descriptor = entry.descriptor.clone();
        match &mut entry.state {
            ComponentState::Unloaded { path, info } => {
                if path == destination {
                    return Err(StableIndexError::Io(
                        "standalone output cannot replace its source file".into(),
                    ));
                }
                let copied = copy_component_file_verified(path, destination, info)
                    .map_err(StableIndexError::Io)?;
                Ok(save_index_info(&copied.descriptor, &copied))
            }
            ComponentState::Loaded { tree, dirty } => {
                let payload = std::mem::replace(tree, memory::new_tree());
                let payload = memory::tree_payload(payload);
                let entry_count = tree_header_entry_count(&payload);
                let result =
                    codec::write_component_file(destination, &descriptor, &payload, entry_count);
                let restored = load_tree_catching(payload)?;
                *tree = restored;
                let info = result.map_err(StableIndexError::Io)?;
                let _ = dirty;
                entry.saved_info = Some(info.clone());
                Ok(save_index_info(&descriptor, &info))
            }
            ComponentState::Failed(error) => Err(StableIndexError::InvalidState(format!(
                "component {} failed and cannot be saved: {error}",
                component_id.0
            ))),
        }
    }

    pub fn open_snapshot(generation_directory: impl AsRef<Path>) -> Result<Self, StableIndexError> {
        let directory = generation_directory.as_ref();
        let manifest = snapshot::read_manifest(directory).map_err(StableIndexError::Format)?;
        let mut prototype = Self {
            repository_identity: parse_hex_32(&manifest.repository_identity)
                .map_err(StableIndexError::Format)?,
            configuration_identity: parse_hex_32(&manifest.configuration_identity)
                .map_err(StableIndexError::Format)?,
            schema_fingerprint: manifest
                .schema_fingerprint
                .as_deref()
                .map(parse_hex_32)
                .transpose()
                .map_err(StableIndexError::Format)?,
            slot_namespace: manifest.slot_namespace.clone(),
            collections: BTreeMap::new(),
            components: BTreeMap::new(),
            generation: Some(directory.to_path_buf()),
            manifest: Some(manifest.clone()),
            counters: PrototypeCounters {
                manifest_opens: 1,
                ..PrototypeCounters::default()
            },
            usable: true,
            build_records: Vec::new(),
        };
        let mut descriptors = Vec::new();
        for component in &manifest.components {
            let path = snapshot::safe_join(directory, &component.path)
                .map_err(StableIndexError::Format)?;
            let info = codec::read_component_header(&path).map_err(StableIndexError::Format)?;
            if info.descriptor != component.descriptor {
                return Err(StableIndexError::Format(format!(
                    "component {} embedded descriptor differs from the manifest",
                    component.id.0
                )));
            }
            if info.descriptor.repository_identity != prototype.repository_identity
                || info.descriptor.configuration_identity != prototype.configuration_identity
                || info.descriptor.slot_namespace != prototype.slot_namespace
            {
                return Err(StableIndexError::Format(format!(
                    "component {} does not belong to this repository configuration",
                    component.id.0
                )));
            }
            if codec::hex_digest(&codec::digest(&info.descriptor_bytes))
                != component.descriptor_sha256
            {
                return Err(StableIndexError::Format(format!(
                    "component {} descriptor hash differs from the manifest",
                    component.id.0
                )));
            }
            if info.envelope.payload_len != component.payload_len
                || info.envelope.entry_count != component.entry_count
                || codec::hex_digest(&info.envelope.payload_digest) != component.payload_sha256
            {
                return Err(StableIndexError::Format(format!(
                    "component {} lengths or checksum differ from the manifest",
                    component.id.0
                )));
            }
            descriptors.push((component.id, info.descriptor.clone()));
            prototype.components.insert(
                component.id,
                ComponentEntry {
                    descriptor: info.descriptor.clone(),
                    state: ComponentState::Unloaded { path, info },
                    saved_info: None,
                },
            );
        }
        prototype.rebuild_collection_index(&manifest.collections, &descriptors)?;
        Ok(prototype)
    }

    pub fn open_index(path: impl AsRef<Path>) -> Result<StandaloneIndex, StableIndexError> {
        let file = codec::read_component_file(path.as_ref()).map_err(StableIndexError::Format)?;
        let tree = load_tree_catching(file.payload)?;
        if tree.len() != file.info.envelope.entry_count {
            return Err(StableIndexError::Format(
                "standalone component entry count does not match its envelope".into(),
            ));
        }
        Ok(StandaloneIndex {
            descriptor: file.info.descriptor,
            tree: Some(tree),
            source_path: Some(file.info.path),
            dirty: false,
            counters: PrototypeCounters {
                standalone_files_opened: 1,
                component_loads: 1,
                component_payload_bytes_read: file.info.envelope.payload_len,
                ..PrototypeCounters::default()
            },
        })
    }

    pub fn manifest(&self) -> Option<&SnapshotManifest> {
        self.manifest.as_ref()
    }

    fn rebuild_collection_index(
        &mut self,
        manifest_collections: &[ManifestCollection],
        descriptors: &[(ComponentId, ComponentDescriptor)],
    ) -> Result<(), StableIndexError> {
        let mut collections = BTreeMap::new();
        for collection in manifest_collections {
            collections.insert(
                CollectionId(collection.id),
                StableCollection {
                    id: CollectionId(collection.id),
                    name: collection.name.clone(),
                    generation: collection.generation,
                    slot_high_water: collection.slot_high_water,
                    live_records: collection.live_records,
                    identity_policy: collection.identity_policy.clone(),
                    records: ComponentId(u32::MAX),
                    identities: ComponentId(u32::MAX),
                    domains: ComponentId(u32::MAX),
                    fields: BTreeMap::new(),
                    texts: BTreeMap::new(),
                    sorts: BTreeMap::new(),
                    relations: BTreeMap::new(),
                    live_cache: None,
                },
            );
        }
        for (id, descriptor) in descriptors {
            let Some(collection_identity) = &descriptor.collection else {
                continue;
            };
            let collection = collections
                .get_mut(&collection_identity.id)
                .ok_or_else(|| {
                    StableIndexError::Format(format!(
                        "component {} names collection {} absent from the manifest",
                        id.0, collection_identity.id.0
                    ))
                })?;
            match descriptor.kind {
                ComponentKind::Records => collection.records = *id,
                ComponentKind::Identities => collection.identities = *id,
                ComponentKind::Domains => collection.domains = *id,
                ComponentKind::Values | ComponentKind::Lengths | ComponentKind::States => {
                    let key = field_component_key(descriptor)?;
                    let entry = collection.fields.entry(key).or_default();
                    match descriptor.kind {
                        ComponentKind::Values => entry.values = Some(*id),
                        ComponentKind::Lengths => entry.lengths = Some(*id),
                        ComponentKind::States => entry.states = Some(*id),
                        _ => unreachable!(),
                    }
                }
                ComponentKind::NormalGrams | ComponentKind::SkipGrams => {
                    let key = text_component_key(descriptor)?;
                    let entry = collection.texts.entry(key).or_default();
                    match descriptor.kind {
                        ComponentKind::NormalGrams => entry.normal = Some(*id),
                        ComponentKind::SkipGrams => entry.skip = Some(*id),
                        _ => unreachable!(),
                    }
                }
                ComponentKind::SortAscending | ComponentKind::SortDescending => {
                    let key = sort_component_key(descriptor);
                    let options = descriptor.ordering.clone().ok_or_else(|| {
                        StableIndexError::Format("sort component has no ordering descriptor".into())
                    })?;
                    let entry = collection
                        .sorts
                        .entry(key)
                        .or_insert_with(|| SortComponentIds {
                            ascending: ComponentId(u32::MAX),
                            descending: ComponentId(u32::MAX),
                            options: options.clone(),
                        });
                    match descriptor.kind {
                        ComponentKind::SortAscending => entry.ascending = *id,
                        ComponentKind::SortDescending => entry.descending = *id,
                        _ => unreachable!(),
                    }
                }
                ComponentKind::RelationOccurrences
                | ComponentKind::RelationTargets
                | ComponentKind::RelationStates => {
                    let field = descriptor.field.as_ref().ok_or_else(|| {
                        StableIndexError::Format("relation component has no field identity".into())
                    })?;
                    let target_collection = field.binding.clone().ok_or_else(|| {
                        StableIndexError::Format(
                            "relation component has no target collection binding".into(),
                        )
                    })?;
                    let key = RelationComponentKey {
                        path: field.path.clone(),
                        target_collection: target_collection.clone(),
                    };
                    let declaration = crate::relation::RelationDefinition {
                        path: field.path.clone(),
                        target_collection: target_collection.clone(),
                        cardinality: crate::relation::RelationCardinality::Many,
                        reverse_alias: None,
                    };
                    let target_id = descriptor.target_collection.ok_or_else(|| {
                        StableIndexError::Format(
                            "relation component has no target collection identifier".into(),
                        )
                    })?;
                    let entry =
                        collection
                            .relations
                            .entry(key)
                            .or_insert_with(|| RelationComponentIds {
                                occurrences: ComponentId(u32::MAX),
                                targets: ComponentId(u32::MAX),
                                states: ComponentId(u32::MAX),
                                declaration,
                                target_collection: target_id,
                            });
                    match descriptor.kind {
                        ComponentKind::RelationOccurrences => entry.occurrences = *id,
                        ComponentKind::RelationTargets => entry.targets = *id,
                        ComponentKind::RelationStates => entry.states = *id,
                        _ => unreachable!(),
                    }
                }
                ComponentKind::Files | ComponentKind::Paths => {}
            }
        }
        for collection in collections.values() {
            if collection.records == ComponentId(u32::MAX)
                || collection.identities == ComponentId(u32::MAX)
                || collection.domains == ComponentId(u32::MAX)
            {
                return Err(StableIndexError::Format(format!(
                    "collection {} is missing a required catalog component",
                    collection.name
                )));
            }
        }
        self.collections = collections;
        Ok(())
    }

    pub(crate) fn live_domain(
        &mut self,
        collection: CollectionId,
    ) -> Result<RoaringBitmap, StableIndexError> {
        if let Some(cached) = self
            .collections
            .get(&collection)
            .and_then(|entry| entry.live_cache.clone())
        {
            return Ok(cached);
        }
        let component = self
            .collections
            .get(&collection)
            .ok_or_else(|| StableIndexError::Format("unknown collection".into()))?
            .domains;
        let tree = self.tree(component)?;
        let value = tree
            .get(&codec::ComponentKey::Domain(DOMAIN_LIVE))
            .ok_or_else(|| StableIndexError::MissingDependency("live-slot domain".to_string()))?;
        let live = memory::decode_bitmap(&value)?;
        self.counters.bitmap_decodes += 1;
        if let Some(entry) = self.collections.get_mut(&collection) {
            entry.live_cache = Some(live.clone());
        }
        Ok(live)
    }

    pub(crate) fn invalidate_collection_cache(&mut self, collection: CollectionId) {
        if let Some(entry) = self.collections.get_mut(&collection) {
            entry.invalidate_live_cache();
        }
    }

    pub fn for_each_raw_entry(
        &mut self,
        id: ComponentId,
        mut visit: impl FnMut(&codec::ComponentKey, &StoredBytes) -> Result<(), String>,
    ) -> Result<(), StableIndexError> {
        let tree = self.tree(id)?;
        for entry in tree.iter() {
            visit(entry.key(), &entry.value()).map_err(StableIndexError::Format)?;
        }
        Ok(())
    }

    pub(crate) fn file_catalog_component(&self) -> Option<ComponentId> {
        self.components
            .values()
            .find(|entry| entry.descriptor.kind == ComponentKind::Files)
            .map(|entry| entry.descriptor.id)
    }

    pub(crate) fn path_catalog_component(&self) -> Option<ComponentId> {
        self.components
            .values()
            .find(|entry| entry.descriptor.kind == ComponentKind::Paths)
            .map(|entry| entry.descriptor.id)
    }
}

pub struct StandaloneIndex {
    descriptor: ComponentDescriptor,
    tree: Option<IndexTree>,
    source_path: Option<PathBuf>,
    dirty: bool,
    counters: PrototypeCounters,
}

impl StandaloneIndex {
    pub fn descriptor(&self) -> &ComponentDescriptor {
        &self.descriptor
    }

    pub fn source_path(&self) -> Option<&Path> {
        self.source_path.as_deref()
    }

    pub fn component_kind(&self) -> ComponentKind {
        self.descriptor.kind
    }

    pub fn counters(&self) -> &PrototypeCounters {
        &self.counters
    }

    pub fn entry_count(&self) -> u64 {
        self.tree.as_ref().map_or(0, |tree| tree.len())
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    fn tree(&self) -> Result<&IndexTree, StableIndexError> {
        self.tree
            .as_ref()
            .ok_or_else(|| StableIndexError::InvalidState("standalone tree is not loaded".into()))
    }

    fn tree_mut(&mut self) -> Result<&mut IndexTree, StableIndexError> {
        self.tree
            .as_mut()
            .ok_or_else(|| StableIndexError::InvalidState("standalone tree is not loaded".into()))
    }

    pub fn validate_key(&self, key: &codec::ComponentKey) -> Result<(), StableIndexError> {
        let expected = match self.descriptor.kind {
            ComponentKind::Files => 0,
            ComponentKind::Paths => 1,
            ComponentKind::Records => 2,
            ComponentKind::Identities => 3,
            ComponentKind::Domains => 4,
            ComponentKind::Values => 5,
            ComponentKind::Lengths => 6,
            ComponentKind::States => 7,
            ComponentKind::NormalGrams | ComponentKind::SkipGrams => 8,
            ComponentKind::SortAscending | ComponentKind::SortDescending => 9,
            ComponentKind::RelationOccurrences => 10,
            ComponentKind::RelationTargets => 11,
            ComponentKind::RelationStates => 12,
        };
        if key.tag() != expected {
            return Err(StableIndexError::Unsupported(format!(
                "key variant {} does not belong to component kind {:?}",
                key.tag(),
                self.descriptor.kind
            )));
        }
        Ok(())
    }

    pub fn raw_get(
        &self,
        key: &codec::ComponentKey,
    ) -> Result<Option<StoredBytes>, StableIndexError> {
        self.validate_key(key)?;
        Ok(self.tree()?.get(key))
    }

    pub fn raw_iter(
        &self,
    ) -> Result<impl Iterator<Item = (codec::ComponentKey, StoredBytes)> + '_, StableIndexError>
    {
        Ok(self
            .tree()?
            .iter()
            .map(|entry| (entry.key().clone(), entry.value())))
    }

    pub fn raw_range(
        &self,
        lower: std::ops::Bound<codec::ComponentKey>,
        upper: std::ops::Bound<codec::ComponentKey>,
    ) -> Result<impl Iterator<Item = (codec::ComponentKey, StoredBytes)> + '_, StableIndexError>
    {
        if let std::ops::Bound::Included(key) | std::ops::Bound::Excluded(key) = &lower {
            self.validate_key(key)?;
        }
        if let std::ops::Bound::Included(key) | std::ops::Bound::Excluded(key) = &upper {
            self.validate_key(key)?;
        }
        Ok(self
            .tree()?
            .range((lower, upper))
            .map(|entry| (entry.key().clone(), entry.value())))
    }

    pub fn raw_posting(
        &self,
        key: &codec::ComponentKey,
    ) -> Result<Option<RoaringBitmap>, StableIndexError> {
        self.raw_get(key)?
            .map(|value| memory::decode_bitmap(&value).map_err(StableIndexError::Format))
            .transpose()
    }

    /// A standalone file intentionally carries no repository liveness or
    /// record metadata.  Repository-semantic operations must not silently
    /// treat every stored slot as live.
    pub fn live_posting(
        &self,
        _key: &codec::ComponentKey,
    ) -> Result<RoaringBitmap, StableIndexError> {
        Err(StableIndexError::MissingDependency(
            "standalone index has no live-slot domain or record catalog".into(),
        ))
    }

    pub fn raw_insert(
        &mut self,
        key: codec::ComponentKey,
        value: StoredBytes,
    ) -> Result<Option<StoredBytes>, StableIndexError> {
        self.validate_key(&key)?;
        let result = self.tree_mut()?.insert(key, value);
        self.dirty = true;
        self.descriptor.detached = true;
        Ok(result)
    }

    pub fn raw_remove(
        &mut self,
        key: &codec::ComponentKey,
    ) -> Result<Option<StoredBytes>, StableIndexError> {
        self.validate_key(key)?;
        let result = self.tree_mut()?.remove(key);
        if result.is_some() {
            self.dirty = true;
            self.descriptor.detached = true;
        }
        Ok(result)
    }

    pub fn save_as(
        &mut self,
        new_file_path: impl AsRef<Path>,
    ) -> Result<SaveIndexInfo, StableIndexError> {
        let destination = new_file_path.as_ref();
        if destination.exists() {
            return Err(StableIndexError::Io(format!(
                "destination {} already exists",
                destination.display()
            )));
        }
        let tree = self.tree.take().ok_or_else(|| {
            StableIndexError::InvalidState("standalone tree is not loaded".into())
        })?;
        let payload = memory::tree_payload(tree);
        let entry_count = tree_header_entry_count(&payload);
        let result =
            codec::write_component_file(destination, &self.descriptor, &payload, entry_count);
        let restored = load_tree_catching(payload)?;
        self.tree = Some(restored);
        let info = result.map_err(StableIndexError::Io)?;
        self.dirty = false;
        self.source_path = Some(info.path.clone());
        Ok(save_index_info(&self.descriptor, &info))
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct SaveIndexInfo {
    pub descriptor: ComponentDescriptor,
    pub path: PathBuf,
    pub payload_len: u64,
    pub entry_count: u64,
    pub descriptor_sha256: String,
    pub payload_sha256: String,
}

fn save_index_info(
    descriptor: &ComponentDescriptor,
    info: &codec::ComponentFileInfo,
) -> SaveIndexInfo {
    SaveIndexInfo {
        descriptor: descriptor.clone(),
        path: info.path.clone(),
        payload_len: info.envelope.payload_len,
        entry_count: info.envelope.entry_count,
        descriptor_sha256: codec::hex_digest(&codec::digest(&info.descriptor_bytes)),
        payload_sha256: codec::hex_digest(&info.envelope.payload_digest),
    }
}

fn manifest_component(
    descriptor: &ComponentDescriptor,
    relative: &str,
    info: &codec::ComponentFileInfo,
) -> Result<ManifestComponent, StableIndexError> {
    Ok(ManifestComponent {
        id: descriptor.id,
        path: relative.into(),
        kind: descriptor.kind,
        descriptor: descriptor.clone(),
        descriptor_sha256: codec::hex_digest(&codec::digest(&info.descriptor_bytes)),
        payload_len: info.envelope.payload_len,
        entry_count: info.envelope.entry_count,
        payload_sha256: codec::hex_digest(&info.envelope.payload_digest),
    })
}

fn tree_header_entry_count(payload: &[u8]) -> u64 {
    if payload.len() >= TREE_HEADER_BYTES {
        u64::from_le_bytes(payload[20..28].try_into().expect("tree header length"))
    } else {
        0
    }
}

fn rewrite_envelope_entry_count(
    path: &Path,
    info: &mut codec::ComponentFileInfo,
    entry_count: u64,
) -> Result<(), String> {
    let mut envelope = info.envelope.clone();
    envelope.entry_count = entry_count;
    let mut file = File::options()
        .write(true)
        .open(path)
        .map_err(|error| format!("reopen component {}: {error}", path.display()))?;
    file.write_all(&envelope.encode())
        .map_err(|error| format!("rewrite entry count: {error}"))?;
    file.flush()
        .map_err(|error| format!("flush entry count: {error}"))?;
    file.sync_all()
        .map_err(|error| format!("sync entry count: {error}"))?;
    info.envelope = envelope;
    Ok(())
}

fn copy_component_file_verified(
    source: &Path,
    destination: &Path,
    expected: &codec::ComponentFileInfo,
) -> Result<codec::ComponentFileInfo, String> {
    let mut input =
        File::open(source).map_err(|error| format!("open {}: {error}", source.display()))?;
    let mut output = File::options()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|error| format!("create {}: {error}", destination.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 256 * 1024];
    let payload_offset = (codec::ENVELOPE_BYTES + expected.envelope.descriptor_len as usize) as u64;
    let mut copied = 0u64;
    loop {
        let read = input
            .read(&mut buffer)
            .map_err(|error| format!("read {}: {error}", source.display()))?;
        if read == 0 {
            break;
        }
        let chunk_start = copied;
        let chunk_end = copied + read as u64;
        let hash_start = payload_offset.max(chunk_start).min(chunk_end);
        if hash_start < chunk_end {
            hasher.update(&buffer[(hash_start - chunk_start) as usize..read]);
        }
        copied = chunk_end;
        output
            .write_all(&buffer[..read])
            .map_err(|error| format!("write {}: {error}", destination.display()))?;
    }
    output
        .flush()
        .map_err(|error| format!("flush {}: {error}", destination.display()))?;
    output
        .sync_all()
        .map_err(|error| format!("sync {}: {error}", destination.display()))?;
    let written: [u8; 32] = hasher.finalize().into();
    if written != expected.envelope.payload_digest {
        return Err("source component payload hash changed during copy".into());
    }
    let info = codec::read_component_header(destination)?;
    if info.envelope != expected.envelope || info.descriptor != expected.descriptor {
        return Err("copied component differs from the verified source".into());
    }
    Ok(info)
}

fn load_tree_catching(payload: Vec<u8>) -> Result<IndexTree, StableIndexError> {
    let result =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| memory::load_tree(payload)));
    match result {
        Ok(Ok(tree)) => Ok(tree),
        Ok(Err(error)) => Err(StableIndexError::Format(error)),
        Err(_) => Err(StableIndexError::Format(
            "stable tree loader panicked on a malformed payload".into(),
        )),
    }
}

fn parse_hex_32(value: &str) -> Result<[u8; 32], String> {
    if value.len() != 64 {
        return Err(format!("expected 64 hex digits, found {}", value.len()));
    }
    let mut bytes = [0u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let text = std::str::from_utf8(pair).map_err(|_| "non-UTF-8 hex digest")?;
        bytes[index] =
            u8::from_str_radix(text, 16).map_err(|_| format!("invalid hex digest byte {text}"))?;
    }
    Ok(bytes)
}

pub fn sort_component_key(descriptor: &ComponentDescriptor) -> SortComponentKey {
    let collection = descriptor
        .collection
        .as_ref()
        .map_or_else(|| "-".to_string(), |value| format!("{:010}", value.id.0));
    let field = descriptor.field.as_ref().map_or_else(
        || "-".to_string(),
        |value| {
            format!(
                "{}/{:?}/{}",
                value.path.join("."),
                value.role,
                value.binding.as_deref().unwrap_or("")
            )
        },
    );
    let ordering = descriptor.ordering.as_ref().map_or_else(
        || "-".to_string(),
        |value| format!("{:?}/{:?}", value.locale, value.options),
    );
    SortComponentKey(format!("{collection}/{field}/{ordering}"))
}

pub(crate) fn field_component_key(
    descriptor: &ComponentDescriptor,
) -> Result<FieldComponentKey, StableIndexError> {
    let field = descriptor
        .field
        .as_ref()
        .ok_or_else(|| StableIndexError::Format("component has no field identity".into()))?;
    Ok(FieldComponentKey {
        path: field.path.clone(),
        binding: field.binding.clone(),
        role: field.role,
    })
}

pub(crate) fn text_component_key(
    descriptor: &ComponentDescriptor,
) -> Result<TextComponentKey, StableIndexError> {
    let field = descriptor
        .field
        .as_ref()
        .ok_or_else(|| StableIndexError::Format("component has no field identity".into()))?;
    Ok(TextComponentKey {
        path: field.path.clone(),
        binding: field.binding.clone(),
        role: field.role,
    })
}

#[cfg(test)]
#[path = "stable_index.test.rs"]
mod tests;
