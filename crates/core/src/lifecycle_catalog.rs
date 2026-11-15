//! Structurally shared file identity and path state for published views.
//!
//! The static repository remains the base. A mutation copies only one small
//! path shard and one file-metadata shard, never the base inventory.

#[cfg(test)]
use std::collections::BTreeSet;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::ops::Bound;
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use roaring::RoaringBitmap;

use crate::inventory::{
    FileEntry, FileStatus, SharedPath, empty_path_bounds, relative_path_bounds,
};
use crate::lifecycle_scan::FileStamp;
use crate::model::FileId;
use crate::query::Operator;
use crate::repository::Repository;

const SHARDS: usize = 1024;


#[derive(Clone)]
struct ChangedFile {
    entry: Arc<FileEntry>,
    fingerprint: Option<[u8; 32]>,
    configuration: Option<[u8; 32]>,
}


type SnapshotSlot = (FileEntry, Option<[u8; 32]>, Option<[u8; 32]>);
type LivenessSlot = (FileEntry, bool, Option<[u8; 32]>, Option<[u8; 32]>);


/// One published view's active file table. Previous views retain their own
/// path map, metadata and liveness after a new root is published.
#[derive(Clone)]
pub(crate) struct FileCatalog {
    base: Arc<Repository>,
    packed_base: Option<Arc<PackedCatalogue>>,
    /// A fresh build can expose its discovered paths before every member is
    /// indexed. Pinned views retain this value across later publications.
    construction_complete: bool,
    base_configuration: [u8; 32],
    changed: Arc<Vec<Arc<HashMap<FileId, ChangedFile>>>>,
    changed_paths: Arc<Vec<Arc<HashMap<SharedPath, FileId>>>>,
    /// Ordered paths that differ from the immutable base; ordinary moves copy
    /// only this delta, while base lookups use the repository's path index.
    changed_relative_paths: Arc<BTreeMap<(Arc<str>, FileId), ()>>,
    active: Arc<RoaringBitmap>,
    len: u32,
}


/// Immutable packed catalogue rows plus a small bounded decode cache. Path and
/// row reads touch only the referenced 64-row page; no file table is rebuilt
/// when a publication is opened.
pub(crate) struct PackedCatalogue {
    view: Arc<crate::index_storage::catalogue::CatalogueView>,
    len: u32,
    cache: Mutex<std::collections::HashMap<u32, Arc<FileEntry>>>,
}


impl PackedCatalogue {
    fn new(view: Arc<crate::index_storage::catalogue::CatalogueView>) -> Self {
        let len = view.high_water();
        Self {
            view,
            len,
            cache: Mutex::new(std::collections::HashMap::new()),
        }
    }

    fn row(&self, file: FileId) -> Option<Arc<FileEntry>> {
        if let Ok(cache) = self.cache.lock()
            && let Some(entry) = cache.get(&file.0)
        {
            return Some(Arc::clone(entry));
        }
        let row = self.view.file(file.0).ok().flatten()?;
        let diagnostics = self.view.diagnostics(&row).ok()?;
        let entry = Arc::new(
            crate::index_storage::packed_restore::file_entry(&row, &diagnostics).ok()?,
        );
        if let Ok(mut cache) = self.cache.lock() {
            if cache.len() >= 4096 {
                cache.clear();
            }
            cache.insert(file.0, Arc::clone(&entry));
        }
        Some(entry)
    }

    fn fingerprint(&self, file: FileId) -> Option<[u8; 32]> {
        if !self.view.is_active(file.0) {
            return None;
        }
        self.view.file(file.0).ok().flatten().and_then(|row| row.fingerprint)
    }

    fn id_for(&self, path: &Path) -> Option<FileId> {
        let query = crate::path_repr::query_path(path);
        self.view.path(query.as_bytes()).ok().flatten().map(FileId)
    }

    fn relative_path_ids(
        &self,
        bounds: (std::ops::Bound<Arc<str>>, std::ops::Bound<Arc<str>>),
        limit: usize,
    ) -> Option<Vec<FileId>> {
        let lower = match &bounds.0 {
            std::ops::Bound::Included(key) => std::ops::Bound::Included(key.as_bytes().to_vec()),
            std::ops::Bound::Excluded(key) => std::ops::Bound::Excluded(key.as_bytes().to_vec()),
            std::ops::Bound::Unbounded => std::ops::Bound::Unbounded,
        };
        let upper = match &bounds.1 {
            std::ops::Bound::Included(key) => std::ops::Bound::Included(key.as_bytes().to_vec()),
            std::ops::Bound::Excluded(key) => std::ops::Bound::Excluded(key.as_bytes().to_vec()),
            std::ops::Bound::Unbounded => std::ops::Bound::Unbounded,
        };
        let mut result = Vec::new();
        for entry in self
            .view
            .path_dictionary()
            .range(owned_bound_ref(&lower), owned_bound_ref(&upper))
        {
            let (_, value) = entry.ok()?;
            let id = u32::from_le_bytes(value.as_slice().try_into().ok()?);
            if id != u32::MAX && self.view.is_active(id) {
                result.push(FileId(id));
                if result.len() == limit {
                    return None;
                }
            }
        }
        Some(result)
    }
}

fn owned_bound_ref(
    bound: &std::ops::Bound<Vec<u8>>,
) -> std::ops::Bound<&[u8]> {
    match bound {
        std::ops::Bound::Included(value) => std::ops::Bound::Included(value.as_slice()),
        std::ops::Bound::Excluded(value) => std::ops::Bound::Excluded(value.as_slice()),
        std::ops::Bound::Unbounded => std::ops::Bound::Unbounded,
    }
}


fn shard<T: Hash>(value: &T) -> usize {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    (hasher.finish() as usize) % SHARDS
}

fn valid_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_)))
}

type IdPathBounds = (Bound<(Arc<str>, FileId)>, Bound<(Arc<str>, FileId)>);

fn path_bounds_with_ids(bounds: &(Bound<Arc<str>>, Bound<Arc<str>>)) -> IdPathBounds {
    let lower = match &bounds.0 {
        Bound::Unbounded => Bound::Unbounded,
        Bound::Included(key) => Bound::Included((Arc::clone(key), FileId(0))),
        Bound::Excluded(key) => Bound::Excluded((Arc::clone(key), FileId(u32::MAX))),
    };
    let upper = match &bounds.1 {
        Bound::Unbounded => Bound::Unbounded,
        Bound::Included(key) => Bound::Included((Arc::clone(key), FileId(u32::MAX))),
        Bound::Excluded(key) => Bound::Excluded((Arc::clone(key), FileId(0))),
    };
    (lower, upper)
}


impl FileCatalog {
    fn configuration_for(base: &Repository) -> Result<[u8; 32], String> {
        let definitions = base
            .collections()
            .iter()
            .map(|collection| collection.definition.clone())
            .collect::<Vec<_>>();
        crate::configuration::configuration_hash(&definitions, base.options(), &base.sources)
    }

    /// Materialize every stable file-ID slot, including retired tombstones.
    /// Maintenance rebases use this so a deleted file in the middle of the
    /// allocated range cannot leave a hole in the packed catalogue.
    pub(crate) fn snapshot_slots_with_liveness(&self) -> Vec<Option<LivenessSlot>> {
        (0..self.len)
            .map(|number| {
                let id = FileId(number);
                if let Some(entry) = self.entry(id) {
                    Some((
                        entry.as_ref().clone(),
                        true,
                        self.fingerprint(id),
                        self.configuration(id),
                    ))
                } else {
                    self.inactive_entry(id)
                        .map(|entry| (entry.as_ref().clone(), false, None, None))
                }
            })
            .collect()
    }

    /// Materialize stable file-ID slots for an explicit maintenance rebase.
    /// Ordinary mutations continue to use the shared sharded representation.
    pub(crate) fn snapshot_slots(&self) -> Vec<Option<SnapshotSlot>> {
        (0..self.len)
            .map(|number| {
                let id = FileId(number);
                self.entry(id)
                    .map(|entry| (entry.as_ref().clone(), self.fingerprint(id), self.configuration(id)))
            })
            .collect()
    }

    /// Rebuild a published catalog from checked cache slots. `None` preserves
    /// a retired file ID, including holes after appended files. Unchanged base
    /// entries continue to borrow their metadata from `base`.
    pub(crate) fn restored(
        base: Arc<Repository>,
        slots: Vec<Option<SnapshotSlot>>,
    ) -> Result<Self, String> {
        let slots = slots
            .into_iter()
            .map(|slot| {
                slot.map(|(entry, fingerprint, configuration)| {
                    (entry, true, fingerprint, configuration)
                })
            })
            .collect();
        Self::restored_with_liveness(base, slots)
    }

    /// Rebuild a published catalog while retaining retired tombstone slots.
    pub(crate) fn restored_with_liveness(
        base: Arc<Repository>,
        slots: Vec<Option<LivenessSlot>>,
    ) -> Result<Self, String> {
        let base_configuration = Self::configuration_for(&base)?;
        let len = u32::try_from(slots.len()).map_err(|_| "too many indexed files")?;
        if slots.len() < base.files().files.len() {
            return Err("restored catalog is shorter than base inventory".into());
        }
        let mut changed = (0..SHARDS).map(|_| HashMap::new()).collect::<Vec<_>>();
        let mut changed_paths = (0..SHARDS).map(|_| HashMap::new()).collect::<Vec<_>>();
        let mut changed_relative_paths = BTreeMap::new();
        let mut active = RoaringBitmap::new();
        let mut paths = HashSet::new();
        for (number, slot) in slots.into_iter().enumerate() {
            let Some((entry, live, fingerprint, configuration)) = slot else {
                continue;
            };
            let id = FileId(number as u32);
            if entry.id != id
                || !valid_relative_path(&entry.relative_path)
                || entry.collections.is_empty()
                || entry
                    .collections
                    .iter()
                    .copied()
                    .collect::<HashSet<_>>()
                    .len()
                    != entry.collections.len()
                || entry
                    .collections
                    .iter()
                    .any(|collection| collection.0 as usize >= base.collections().len())
                || entry.diagnostics.iter().any(|diagnostic| {
                    diagnostic.file != id || !entry.collections.contains(&diagnostic.collection)
                })
                || !paths.insert(entry.relative_path.clone())
            {
                return Err("invalid restored file catalog entry".into());
            }
            let baseline = base.files().files.get(number);
            let new_path =
                baseline.is_none_or(|original| original.relative_path != entry.relative_path);
            let changed_metadata = baseline.is_none_or(|original| {
                original.revision != entry.revision
                    || original.created_ns != entry.created_ns
                    || original.collections != entry.collections
                    || original.status != entry.status
                    || original.diagnostics != entry.diagnostics
                    || original.omitted_diagnostics != entry.omitted_diagnostics
            });
            if new_path && live {
                let path = entry.relative_path.clone();
                changed_relative_paths.insert((entry.relative_path.query_key(), id), ());
                changed_paths[shard(&path)].insert(path, id);
            }
            let base_fingerprint = base.base_fingerprints.get(number).copied().flatten();
            let base_configuration_for_file = base_fingerprint.map(|_| base_configuration);
            if !live
                || new_path
                || changed_metadata
                || fingerprint != base_fingerprint
                || configuration != base_configuration_for_file
            {
                changed[shard(&id)].insert(
                    id,
                    ChangedFile {
                        entry: Arc::new(entry),
                        fingerprint,
                        configuration,
                    },
                );
            }
            if live {
                active.insert(id.0);
            }
        }
        Ok(Self {
            base,
            packed_base: None,
            construction_complete: true,
            base_configuration,
            changed: Arc::new(changed.into_iter().map(Arc::new).collect()),
            changed_paths: Arc::new(changed_paths.into_iter().map(Arc::new).collect()),
            changed_relative_paths: Arc::new(changed_relative_paths),
            active: Arc::new(active),
            len,
        })
    }

    pub(crate) fn from_repository(base: Arc<Repository>) -> Result<Self, String> {
        let base_configuration = Self::configuration_for(&base)?;
        let len = u32::try_from(base.files().files.len()).map_err(|_| "too many indexed files")?;
        let mut active = RoaringBitmap::new();
        active.insert_range(0..len);
        Ok(Self {
            base,
            packed_base: None,
            construction_complete: true,
            base_configuration,
            changed: Arc::new(
                (0..SHARDS)
                    .map(|_| Arc::new(HashMap::new()))
                    .collect::<Vec<_>>(),
            ),
            changed_paths: Arc::new(
                (0..SHARDS)
                    .map(|_| Arc::new(HashMap::new()))
                    .collect::<Vec<_>>(),
            ),
            changed_relative_paths: Arc::new(BTreeMap::new()),
            active: Arc::new(active),
            len,
        })
    }

    /// Open a publication catalogue without materializing its file rows.
    /// `base` must be an empty repository that still carries configuration and
    /// collection metadata; rows and paths are read from the packed object.
    pub(crate) fn from_packed(
        base: Arc<Repository>,
        view: Arc<crate::index_storage::catalogue::CatalogueView>,
    ) -> Result<Self, String> {
        let base_configuration = Self::configuration_for(&base)?;
        let packed = PackedCatalogue::new(view);
        let len = packed.len;
        let active = Arc::new(packed.view.active_files().clone());
        Ok(Self {
            base,
            packed_base: Some(Arc::new(packed)),
            construction_complete: true,
            base_configuration,
            changed: Arc::new(
                (0..SHARDS)
                    .map(|_| Arc::new(HashMap::new()))
                    .collect::<Vec<_>>(),
            ),
            changed_paths: Arc::new(
                (0..SHARDS)
                    .map(|_| Arc::new(HashMap::new()))
                    .collect::<Vec<_>>(),
            ),
            changed_relative_paths: Arc::new(BTreeMap::new()),
            active,
            len,
        })
    }

    pub(crate) fn allocated_files(&self) -> usize {
        self.len as usize
    }

    pub(crate) fn construction_complete(&self) -> bool {
        self.construction_complete
    }

    pub(crate) fn repository_root(&self) -> &Path {
        &self.base.files().root
    }

    pub(crate) fn set_construction_complete(&mut self, complete: bool) {
        self.construction_complete = complete;
    }

    /// Rebind an otherwise unchanged discovered inventory to final metadata
    /// (global inference and captured fingerprints) without copying paths.
    pub(crate) fn rebase(&mut self, base: Arc<Repository>) -> Result<(), String> {
        if !Arc::ptr_eq(&self.base.files, &base.files)
            || Self::configuration_for(&base)? != self.base_configuration
        {
            return Err("catalog rebase changed discovered files or configuration".into());
        }
        if !self.construction_complete {
            // Initial diagnostic overlays precede final digest assembly. Give
            // them the same captured fingerprint as unchanged base entries.
            let shards = Arc::make_mut(&mut self.changed);
            for shard in shards.iter_mut().filter(|shard| !shard.is_empty()) {
                for (id, changed) in Arc::make_mut(shard).iter_mut() {
                    changed.fingerprint =
                        base.base_fingerprints.get(id.0 as usize).copied().flatten();
                    changed.configuration = changed.fingerprint.map(|_| self.base_configuration);
                }
            }
        }
        self.base = base;
        Ok(())
    }

    pub(crate) fn active_files(&self) -> u64 {
        self.active.len()
    }

    pub(crate) fn is_active(&self, file: FileId) -> bool {
        self.active.contains(file.0)
    }

    pub(crate) fn entry(&self, file: FileId) -> Option<Arc<FileEntry>> {
        if !self.is_active(file) {
            return None;
        }
        if let Some(changed) = self.changed[shard(&file)].get(&file) {
            return Some(Arc::clone(&changed.entry));
        }
        if let Some(packed) = &self.packed_base {
            return packed.row(file);
        }
        self.base
            .files()
            .files
            .get(file.0 as usize)
            .map(|entry| Arc::new(entry.clone()))
    }

    /// Entry metadata for one retired file slot, retained as a tombstone.
    pub(crate) fn inactive_entry(&self, file: FileId) -> Option<Arc<FileEntry>> {
        if self.active.contains(file.0) {
            return None;
        }
        if let Some(changed) = self.changed[shard(&file)].get(&file) {
            return Some(Arc::clone(&changed.entry));
        }
        if let Some(packed) = &self.packed_base {
            return packed.row(file);
        }
        self.base
            .files()
            .files
            .get(file.0 as usize)
            .map(|entry| Arc::new(entry.clone()))
    }

    pub(crate) fn fingerprint(&self, file: FileId) -> Option<[u8; 32]> {
        if !self.is_active(file) {
            return None;
        }
        self.changed[shard(&file)]
            .get(&file)
            .map(|changed| changed.fingerprint)
            .unwrap_or_else(|| {
                if let Some(packed) = &self.packed_base {
                    return packed.fingerprint(file);
                }
                self.base
                    .base_fingerprints
                    .get(file.0 as usize)
                    .copied()
                    .flatten()
            })
    }

    pub(crate) fn configuration(&self, file: FileId) -> Option<[u8; 32]> {
        if !self.is_active(file) {
            return None;
        }
        self.changed[shard(&file)]
            .get(&file)
            .map(|changed| changed.configuration)
            .unwrap_or_else(|| self.fingerprint(file).map(|_| self.base_configuration))
    }

    pub(crate) fn id_for(&self, path: &Path) -> Option<FileId> {
        let id = self.changed_paths[shard(&path)]
            .get(path)
            .copied()
            .or_else(|| {
                if let Some(packed) = &self.packed_base {
                    return packed.id_for(path);
                }
                self.base.files().id_for(path)
            })?;
        if !self.is_active(id) {
            return None;
        }
        if let Some(changed) = self.changed[shard(&id)].get(&id) {
            return (changed.entry.relative_path.as_path() == path).then_some(id);
        }
        if let Some(packed) = &self.packed_base {
            return (packed.id_for(path) == Some(id)).then_some(id);
        }
        self.base
            .files()
            .files
            .get(id.0 as usize)
            .filter(|entry| entry.relative_path.as_path() == path)
            .map(|_| id)
    }

    /// Exact file IDs for an ordered `@file.path` comparison in this pinned
    /// publication, including additions and content-preserving moves.
    #[cfg(test)]
    pub(crate) fn relative_path_ids(
        &self,
        operator: Operator,
        literal: &str,
    ) -> Option<Vec<FileId>> {
        let bounds = relative_path_bounds(operator, literal)?;
        Some(self.relative_path_ids_in_bounds(bounds))
    }

    pub(crate) fn relative_path_ids_in_bounds(
        &self,
        bounds: (std::ops::Bound<Arc<str>>, std::ops::Bound<Arc<str>>),
    ) -> Vec<FileId> {
        self.relative_path_ids_in_bounds_up_to(bounds, usize::MAX)
            .expect("an index cannot contain usize::MAX paths")
    }

    pub(crate) fn relative_path_ids_up_to(
        &self,
        operator: Operator,
        literal: &str,
        limit: usize,
    ) -> Option<Vec<FileId>> {
        let bounds = relative_path_bounds(operator, literal)?;
        self.relative_path_ids_in_bounds_up_to(bounds, limit)
    }

    fn relative_path_ids_in_bounds_up_to(
        &self,
        bounds: (std::ops::Bound<Arc<str>>, std::ops::Bound<Arc<str>>),
        limit: usize,
    ) -> Option<Vec<FileId>> {
        if empty_path_bounds(&bounds) {
            return Some(Vec::new());
        }
        if let Some(packed) = &self.packed_base {
            return packed.relative_path_ids(
                (
                    bounds.0.map(|key| Arc::clone(&key)),
                    bounds.1.map(|key| Arc::clone(&key)),
                ),
                limit,
            );
        }
        let (mut ids, mut scanned) = {
            let mut ids = Vec::new();
            let mut scanned = 0usize;
            for (_, id) in self.base.files().relative_paths.range(bounds.clone()) {
                if scanned == limit {
                    return None;
                }
                scanned += 1;
                if self.is_active(*id)
                    && self.changed[shard(id)].get(id).is_none_or(|changed| {
                        self.base
                            .files()
                            .files
                            .get(id.0 as usize)
                            .is_some_and(|baseline| {
                                changed.entry.relative_path == baseline.relative_path
                            })
                    })
                {
                    ids.push(*id);
                }
            }
            (ids, scanned)
        };
        for ((key, id), _) in self
            .changed_relative_paths
            .range(path_bounds_with_ids(&bounds))
        {
            if scanned == limit {
                return None;
            }
            scanned += 1;
            if self.is_active(*id)
                && self.changed[shard(id)]
                    .get(id)
                    .is_some_and(|changed| changed.entry.relative_path.query_key() == *key)
            {
                ids.push(*id);
            }
        }
        Some(ids)
    }

    /// Materialize the active metadata baseline only for a full filesystem
    /// scan. Ordinary mutations and query paths never traverse this table.
    #[cfg(test)]
    pub(crate) fn scan_stamps(&self) -> BTreeMap<PathBuf, FileStamp> {
        let mut stamps = BTreeMap::new();
        for number in self.active.iter() {
            let Some(entry) = self.entry(FileId(number)) else {
                continue;
            };
            stamps.insert(
                entry.relative_path.to_path_buf(),
                FileStamp {
                    path: entry.relative_path.to_path_buf(),
                    revision: entry.revision,
                    created_ns: entry.created_ns,
                    collections: entry.collections.iter().copied().collect::<BTreeSet<_>>(),
                    retry_unreadable: entry.status == FileStatus::Unreadable,
                },
            );
        }
        stamps
    }

    pub(crate) fn scan_stamp(&self, file: FileId) -> Option<FileStamp> {
        let entry = self.entry(file)?;
        Some(FileStamp {
            path: entry.relative_path.to_path_buf(),
            revision: entry.revision,
            created_ns: entry.created_ns,
            collections: entry.collections.iter().copied().collect(),
            retry_unreadable: entry.status == FileStatus::Unreadable,
        })
    }

    pub(crate) fn active_ids(&self) -> impl Iterator<Item = FileId> + '_ {
        self.active.iter().map(FileId)
    }

    pub(crate) fn retired(&self, file: FileId) -> Result<Self, String> {
        let entry = self.entry(file).ok_or("unknown active file")?;
        let mut next = self.clone();
        Arc::make_mut(&mut next.active).remove(file.0);
        let bucket = shard(&entry.relative_path);
        let paths = Arc::make_mut(&mut next.changed_paths);
        Arc::make_mut(&mut paths[bucket]).remove(&entry.relative_path);
        Arc::make_mut(&mut next.changed_relative_paths)
            .remove(&(entry.relative_path.query_key(), file));
        Ok(next)
    }

    pub(crate) fn appended(
        &self,
        entry: FileEntry,
        fingerprint: Option<[u8; 32]>,
        configuration: Option<[u8; 32]>,
    ) -> Result<Self, String> {
        if entry.id.0 != self.len
            || !valid_relative_path(&entry.relative_path)
            || self.id_for(&entry.relative_path).is_some()
        {
            return Err("file ID or path collision".into());
        }
        let mut next = self.clone();
        let id = entry.id;
        let path = entry.relative_path.clone();
        let query_key = path.query_key();
        let changed = Arc::make_mut(&mut next.changed);
        Arc::make_mut(&mut changed[shard(&id)]).insert(
            id,
            ChangedFile {
                entry: Arc::new(entry),
                fingerprint,
                configuration,
            },
        );
        let paths = Arc::make_mut(&mut next.changed_paths);
        Arc::make_mut(&mut paths[shard(&path)]).insert(path, id);
        Arc::make_mut(&mut next.changed_relative_paths).insert((query_key, id), ());
        Arc::make_mut(&mut next.active).insert(id.0);
        next.len = next.len.checked_add(1).ok_or("too many indexed files")?;
        Ok(next)
    }

    pub(crate) fn replaced(
        &self,
        entry: FileEntry,
        fingerprint: Option<[u8; 32]>,
        configuration: Option<[u8; 32]>,
    ) -> Result<Self, String> {
        let id = entry.id;
        let old_path = self
            .entry(id)
            .ok_or("unknown active file")?
            .relative_path
            .clone();
        if !valid_relative_path(&entry.relative_path) {
            return Err("file path must be relative and contained".into());
        }
        if self
            .id_for(&entry.relative_path)
            .is_some_and(|other| other != id)
        {
            return Err("file path collision".into());
        }
        let mut next = self.clone();
        let new_path = entry.relative_path.clone();
        let changed = Arc::make_mut(&mut next.changed);
        Arc::make_mut(&mut changed[shard(&id)]).insert(
            id,
            ChangedFile {
                entry: Arc::new(entry),
                fingerprint,
                configuration,
            },
        );
        let paths = Arc::make_mut(&mut next.changed_paths);
        Arc::make_mut(&mut paths[shard(&old_path)]).remove(&old_path);
        Arc::make_mut(&mut paths[shard(&new_path)]).insert(new_path.clone(), id);
        let ordered = Arc::make_mut(&mut next.changed_relative_paths);
        ordered.remove(&(old_path.query_key(), id));
        let matches_base = next
            .base
            .files()
            .files
            .get(id.0 as usize)
            .is_some_and(|baseline| baseline.relative_path == new_path);
        if !matches_base {
            ordered.insert((new_path.query_key(), id), ());
        }
        Ok(next)
    }
}


#[cfg(test)]
#[path = "lifecycle_catalog.test.rs"]
mod tests;
