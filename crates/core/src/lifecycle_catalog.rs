//! Structurally shared file identity and path state for published views.
//!
//! The static repository remains the base. A mutation copies only one small
//! path shard and one file-metadata shard, never the base inventory.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use roaring::RoaringBitmap;

use crate::inventory::{FileEntry, FileStatus};
use crate::lifecycle_scan::FileStamp;
use crate::model::FileId;
use crate::repository::Repository;

const SHARDS: usize = 1024;


#[derive(Clone)]
struct ChangedFile {
    entry: Arc<FileEntry>,
    fingerprint: Option<[u8; 32]>,
    configuration: Option<[u8; 32]>,
}


/// One published view's active file table. Previous views retain their own
/// path map, metadata and liveness after a new root is published.
#[derive(Clone)]
pub(crate) struct FileCatalog {
    base: Arc<Repository>,
    changed: Arc<Vec<Arc<HashMap<FileId, ChangedFile>>>>,
    changed_paths: Arc<Vec<Arc<HashMap<PathBuf, FileId>>>>,
    active: Arc<RoaringBitmap>,
    len: u32,
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


impl FileCatalog {
    /// Rebuild a published catalog from checked cache slots. `None` preserves
    /// a retired file ID, including holes after appended files. Unchanged base
    /// entries continue to borrow their metadata from `base`.
    pub(crate) fn restored(
        base: Arc<Repository>,
        slots: Vec<Option<(FileEntry, Option<[u8; 32]>, Option<[u8; 32]>)>>,
    ) -> Result<Self, String> {
        let len = u32::try_from(slots.len()).map_err(|_| "too many indexed files")?;
        if slots.len() < base.files().files.len() {
            return Err("restored catalog is shorter than base inventory".into());
        }
        let mut changed = (0..SHARDS).map(|_| HashMap::new()).collect::<Vec<_>>();
        let mut changed_paths = (0..SHARDS).map(|_| HashMap::new()).collect::<Vec<_>>();
        let mut active = RoaringBitmap::new();
        let mut paths = HashSet::new();
        for (number, slot) in slots.into_iter().enumerate() {
            let Some((entry, fingerprint, configuration)) = slot else {
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
            if new_path {
                let path = entry.relative_path.clone();
                changed_paths[shard(&path)].insert(path, id);
            }
            if new_path || changed_metadata || fingerprint.is_some() || configuration.is_some() {
                changed[shard(&id)].insert(
                    id,
                    ChangedFile {
                        entry: Arc::new(entry),
                        fingerprint,
                        configuration,
                    },
                );
            }
            active.insert(id.0);
        }
        Ok(Self {
            base,
            changed: Arc::new(changed.into_iter().map(Arc::new).collect()),
            changed_paths: Arc::new(changed_paths.into_iter().map(Arc::new).collect()),
            active: Arc::new(active),
            len,
        })
    }

    pub(crate) fn from_repository(base: Arc<Repository>) -> Result<Self, String> {
        let len = u32::try_from(base.files().files.len()).map_err(|_| "too many indexed files")?;
        let mut active = RoaringBitmap::new();
        active.insert_range(0..len);
        Ok(Self {
            base,
            changed: Arc::new(vec![Arc::new(HashMap::new()); SHARDS]),
            changed_paths: Arc::new(vec![Arc::new(HashMap::new()); SHARDS]),
            active: Arc::new(active),
            len,
        })
    }

    pub(crate) fn allocated_files(&self) -> usize {
        self.len as usize
    }

    pub(crate) fn active_files(&self) -> u64 {
        self.active.len()
    }

    pub(crate) fn is_active(&self, file: FileId) -> bool {
        self.active.contains(file.0)
    }

    pub(crate) fn entry(&self, file: FileId) -> Option<&FileEntry> {
        if !self.is_active(file) {
            return None;
        }
        self.changed[shard(&file)]
            .get(&file)
            .map(|changed| changed.entry.as_ref())
            .or_else(|| self.base.files().files.get(file.0 as usize))
    }

    pub(crate) fn fingerprint(&self, file: FileId) -> Option<[u8; 32]> {
        if !self.is_active(file) {
            return None;
        }
        self.changed[shard(&file)]
            .get(&file)
            .and_then(|changed| changed.fingerprint)
    }

    pub(crate) fn configuration(&self, file: FileId) -> Option<[u8; 32]> {
        if !self.is_active(file) {
            return None;
        }
        self.changed[shard(&file)]
            .get(&file)
            .and_then(|changed| changed.configuration)
    }

    pub(crate) fn id_for(&self, path: &Path) -> Option<FileId> {
        let id = self.changed_paths[shard(&path)]
            .get(path)
            .copied()
            .or_else(|| self.base.files().id_for(path))?;
        self.entry(id)
            .filter(|entry| entry.relative_path == path)
            .map(|_| id)
    }

    /// Materialize the active metadata baseline only for a full filesystem
    /// scan. Ordinary mutations and query paths never traverse this table.
    pub(crate) fn scan_stamps(&self) -> BTreeMap<PathBuf, FileStamp> {
        let mut stamps = BTreeMap::new();
        for number in self.active.iter() {
            let Some(entry) = self.entry(FileId(number)) else {
                continue;
            };
            stamps.insert(
                entry.relative_path.clone(),
                FileStamp {
                    path: entry.relative_path.clone(),
                    revision: entry.revision,
                    created_ns: entry.created_ns,
                    collections: entry.collections.iter().copied().collect::<BTreeSet<_>>(),
                    retry_unreadable: entry.status == FileStatus::Unreadable,
                },
            );
        }
        stamps
    }

    pub(crate) fn retired(&self, file: FileId) -> Result<Self, String> {
        let entry = self.entry(file).ok_or("unknown active file")?;
        let mut next = self.clone();
        Arc::make_mut(&mut next.active).remove(file.0);
        let bucket = shard(&entry.relative_path);
        let paths = Arc::make_mut(&mut next.changed_paths);
        Arc::make_mut(&mut paths[bucket]).remove(&entry.relative_path);
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
        Arc::make_mut(&mut paths[shard(&new_path)]).insert(new_path, id);
        Ok(next)
    }
}


#[cfg(test)]
#[path = "lifecycle_catalog.test.rs"]
mod tests;
