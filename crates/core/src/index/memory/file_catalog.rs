//! Structurally shared file identity and path state for published views.
//!
//! The static repository remains the base. A mutation copies only one small
//! path shard and one file-metadata shard, never the base inventory.

use std::borrow::Cow;
#[cfg(test)]
use std::collections::BTreeSet;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::ops::Bound;
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;
use std::sync::Arc;

use roaring::RoaringBitmap;

use super::inventory::{FileInventory, FileVersions};
use crate::index::storage::file_catalog::{
    DiscoveredFile, DiscoveredFileRows, FILE_TRANSFER_LIMIT, FileCatalogBase, FileCatalogDelta,
    FileCatalogEditor, FileCatalogHandle, FileCatalogReader, FileCatalogRevision,
    FileCatalogSource, FileDiscoveryBuilder, FilePathBounds, FileScalarPredicate, FileTransferRow,
    transfer_files,
};
use crate::index::storage::transfer::{BatchLimit, BatchStatus, StorageError, StorageResult};
#[cfg(test)]
use crate::inventory::FileStatus;
#[cfg(test)]
use crate::inventory::relative_path_bounds;
use crate::inventory::{FileEntry, FileVersion, SharedPath, empty_path_bounds};
#[cfg(test)]
use crate::lifecycle::reconciliation::scan::FileStamp;
use crate::model::{CollectionId, FileId, Number};
use crate::query::Operator;
#[cfg(test)]
use crate::repository::Repository;

use super::file_scalar::FileScalarIndex;

const SHARDS: usize = 256;
const SCALAR_SHARDS: usize = 32;
const SCALAR_CACHE_LIMIT: usize = 64;

#[derive(Clone, Eq, Ord, PartialEq, PartialOrd)]
enum ScalarLookup {
    Size(u8, Number, usize),
    Modified(u8, i128, usize),
    Created(u8, i128, usize),
    UnknownCreated(usize),
}

fn operator_key(operator: Operator) -> Option<u8> {
    Some(match operator {
        Operator::Eq => 0,
        Operator::Ne => 1,
        Operator::Gt => 2,
        Operator::Ge => 3,
        Operator::Lt => 4,
        Operator::Le => 5,
        _ => return None,
    })
}

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
    base_files: Arc<FileInventory>,
    base_fingerprints: Arc<Vec<Option<[u8; 32]>>>,
    /// A fresh build can expose its discovered paths before every member is
    /// indexed. Pinned views retain this value across later publications.
    construction_complete: bool,
    base_configuration: [u8; 32],
    changed: Arc<Vec<Arc<HashMap<FileId, ChangedFile>>>>,
    changed_paths: Arc<Vec<Arc<HashMap<SharedPath, FileId>>>>,
    /// Ordered paths that differ from the immutable base; ordinary moves copy
    /// only this delta, while base lookups use the repository's path index.
    changed_relative_paths: Arc<BTreeMap<(Arc<str>, FileId), ()>>,
    /// Only changed file scalar memberships live here; the immutable base
    /// keeps one sorted membership per discovered physical file.
    changed_scalar: Arc<Vec<Arc<FileScalarIndex>>>,
    scalar_cache: Arc<std::sync::Mutex<BTreeMap<ScalarLookup, Option<Arc<[FileId]>>>>>,
    active: Arc<RoaringBitmap>,
    scalar_base_live: Arc<RoaringBitmap>,
    len: u32,
    versions: FileVersions,
}

/// Native discoveries are retained by reference. Other representations can
/// expose the same discovered base by reading its bounded row source.
#[cfg(test)]
struct DiscoveredFiles {
    inventory: Arc<FileInventory>,
    fingerprints: Arc<Vec<Option<[u8; 32]>>>,
    configuration: [u8; 32],
    collections: usize,
}

#[cfg(test)]
impl FileInventory {
    pub(crate) fn catalog_base(
        self: &Arc<Self>,
        fingerprints: Vec<Option<[u8; 32]>>,
        configuration: [u8; 32],
        collections: usize,
    ) -> impl FileCatalogBase + use<> {
        DiscoveredFiles {
            inventory: Arc::clone(self),
            fingerprints: Arc::new(fingerprints),
            configuration,
            collections,
        }
    }
}

#[cfg(test)]
impl FileCatalogBase for DiscoveredFiles {
    fn root(&self) -> Option<&Path> {
        self.inventory.root()
    }

    fn collection_count(&self) -> usize {
        self.collections
    }

    fn configuration(&self) -> [u8; 32] {
        self.configuration
    }
}

#[cfg(test)]
impl FileCatalogSource for DiscoveredFiles {
    fn high_water_mark(&self) -> u32 {
        u32::try_from(self.inventory.files.len()).expect("checked file inventory length")
    }

    fn live_count(&self) -> u64 {
        self.high_water_mark() as u64
    }

    fn read_batch<'a>(
        &'a self,
        after: Option<FileId>,
        limit: BatchLimit,
        output: &mut Vec<FileTransferRow<'a>>,
    ) -> StorageResult<BatchStatus> {
        let start = after.map_or(0, |id| id.0 as usize + 1);
        let mut bytes = 0;
        for file in self.inventory.files.iter().skip(start) {
            let fingerprint = self.fingerprints.get(file.id.0 as usize).copied().flatten();
            let row = FileTransferRow {
                entry: Cow::Borrowed(file),
                fingerprint,
                configuration: fingerprint.map(|_| self.configuration),
            };
            let size = row.bytes();
            if !limit.accepts("read discovered files", output.len(), bytes, size)? {
                return Ok(BatchStatus::More);
            }
            bytes += size;
            output.push(row);
        }
        Ok(BatchStatus::Done)
    }
}

impl FileDiscoveryBuilder for FileInventory {
    fn register(
        &mut self,
        collection: CollectionId,
        files: &mut Vec<DiscoveredFile>,
        members: &mut Vec<FileId>,
    ) -> StorageResult<()> {
        for file in files.drain(..) {
            let id = self
                .add(file.relative, file.revision, file.created_ns, collection)
                .map_err(|error| StorageError::invalid("register discovered file", error))?;
            members.push(id);
        }
        Ok(())
    }

    fn seal(self: Box<Self>, configuration: [u8; 32]) -> StorageResult<FileCatalogHandle> {
        self.validate_diagnostics()
            .map_err(|error| StorageError::invalid("seal discovered files", error))?;
        Ok(Arc::new(FileCatalog::from_parts(
            Arc::new(*self),
            Arc::new(Vec::new()),
            configuration,
            FileVersion::INITIAL,
        )))
    }
}

fn native_base(
    base: &dyn FileCatalogBase,
) -> StorageResult<(Arc<FileInventory>, Arc<Vec<Option<[u8; 32]>>>)> {
    #[cfg(test)]
    if let Some(base) = (base as &dyn std::any::Any).downcast_ref::<DiscoveredFiles>() {
        return Ok((Arc::clone(&base.inventory), Arc::clone(&base.fingerprints)));
    }

    let mut inventory = base
        .root()
        .map_or_else(FileInventory::without_root, |root| {
            FileInventory::new(root.to_path_buf())
        });
    let mut fingerprints = Vec::new();
    transfer_files(
        base,
        base.high_water_mark(),
        FILE_TRANSFER_LIMIT,
        &|| false,
        |row| {
            if row.entry.id.0 as usize != inventory.files.len() {
                return Err(StorageError::invalid(
                    "load discovered base",
                    "non-contiguous file IDs",
                ));
            }
            inventory.files.push(row.entry.into_owned());
            fingerprints.push(row.fingerprint);
            Ok(())
        },
    )?;
    inventory
        .rebuild_lookup()
        .map_err(|error| StorageError::invalid("load discovered base", error))?;
    Ok((Arc::new(inventory), Arc::new(fingerprints)))
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

fn same_scalars(left: &FileEntry, right: &FileEntry) -> bool {
    left.revision.size == right.revision.size
        && left.revision.modified_ns == right.revision.modified_ns
        && left.created_ns == right.created_ns
}

fn path_bounds_with_ids(
    bounds: &(Bound<Arc<str>>, Bound<Arc<str>>),
) -> (Bound<(Arc<str>, FileId)>, Bound<(Arc<str>, FileId)>) {
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
    pub(super) fn from_base(
        base: &dyn FileCatalogBase,
        version: FileVersion,
    ) -> StorageResult<Self> {
        let (files, fingerprints) = native_base(base)?;
        Ok(Self::from_parts(
            files,
            fingerprints,
            base.configuration(),
            version,
        ))
    }

    /// Build a private catalog from ordered live rows and an allocation bound.
    /// Missing IDs stay retired, including trailing holes.
    pub(super) fn restore_base(
        base: &dyn FileCatalogBase,
        source: &dyn FileCatalogSource,
        cancelled: &dyn Fn() -> bool,
        versions: FileVersions,
        construction_complete: bool,
    ) -> StorageResult<Self> {
        const OP: &str = "restore file catalog";
        let (base_files, base_fingerprints) = native_base(base)?;
        let base_configuration = base.configuration();
        let len = source.high_water_mark();
        if (len as usize) < base_files.files.len() {
            return Err(StorageError::invalid(
                OP,
                "source is shorter than base inventory",
            ));
        }
        let mut changed = (0..SHARDS).map(|_| HashMap::new()).collect::<Vec<_>>();
        let mut changed_paths = (0..SHARDS).map(|_| HashMap::new()).collect::<Vec<_>>();
        let mut changed_relative_paths = BTreeMap::new();
        let mut changed_scalar = vec![FileScalarIndex::default(); SCALAR_SHARDS];
        let mut active = RoaringBitmap::new();
        let mut scalar_base_live = RoaringBitmap::new();
        let mut paths = HashSet::new();
        transfer_files(source, len, FILE_TRANSFER_LIMIT, cancelled, |row| {
            let entry = row.entry.as_ref();
            let id = entry.id;
            let number = id.0 as usize;
            let FileTransferRow {
                fingerprint,
                configuration,
                ..
            } = row;
            if !valid_relative_path(&entry.relative_path)
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
                    .any(|collection| collection.0 as usize >= base.collection_count())
                || entry.diagnostics.iter().any(|diagnostic| {
                    diagnostic.file != id || !entry.collections.contains(&diagnostic.collection)
                })
                || !paths.insert(entry.relative_path.clone())
            {
                return Err(StorageError::invalid(
                    OP,
                    format!("invalid entry for file ID {}", id.0),
                ));
            }
            let baseline = base_files.files.get(number);
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
                changed_relative_paths.insert((entry.relative_path.query_key(), id), ());
                changed_paths[shard(&path)].insert(path, id);
            }
            let base_fingerprint = base_fingerprints.get(number).copied().flatten();
            let base_configuration_for_file = base_fingerprint.map(|_| base_configuration);
            if baseline.is_some_and(|base| same_scalars(base, entry)) {
                scalar_base_live.insert(id.0);
            } else {
                changed_scalar[id.0 as usize % SCALAR_SHARDS].insert(entry);
            }
            if new_path
                || changed_metadata
                || fingerprint != base_fingerprint
                || configuration != base_configuration_for_file
            {
                changed[shard(&id)].insert(
                    id,
                    ChangedFile {
                        entry: Arc::new(row.entry.into_owned()),
                        fingerprint,
                        configuration,
                    },
                );
            }
            active.insert(id.0);
            Ok(())
        })?;
        Ok(Self {
            base_files,
            base_fingerprints,
            construction_complete,
            base_configuration,
            changed: Arc::new(changed.into_iter().map(Arc::new).collect()),
            changed_paths: Arc::new(changed_paths.into_iter().map(Arc::new).collect()),
            changed_relative_paths: Arc::new(changed_relative_paths),
            changed_scalar: Arc::new(changed_scalar.into_iter().map(Arc::new).collect()),
            scalar_cache: Arc::new(std::sync::Mutex::new(BTreeMap::new())),
            active: Arc::new(active),
            scalar_base_live: Arc::new(scalar_base_live),
            len,
            versions,
        })
    }

    pub(super) fn restore_revision(
        base: &dyn FileCatalogBase,
        source: &dyn FileCatalogRevision,
        cancelled: &dyn Fn() -> bool,
    ) -> StorageResult<Self> {
        let versions = match (source as &dyn std::any::Any).downcast_ref::<Self>() {
            Some(source) => source.versions.clone(),
            None => FileVersions::from_catalog(source, cancelled)?,
        };
        Self::restore_base(
            base,
            source,
            cancelled,
            versions,
            source.construction_complete(),
        )
    }

    #[cfg(test)]
    pub(crate) fn from_repository(base: Arc<Repository>) -> Result<Self, String> {
        Self::from_base(&base.file_catalog_base(), FileVersion::INITIAL).map_err(String::from)
    }

    #[cfg(test)]
    fn restore_from(
        base: &Repository,
        source: &dyn FileCatalogSource,
        cancelled: &dyn Fn() -> bool,
    ) -> StorageResult<Self> {
        Self::restore_base(
            &base.file_catalog_base(),
            source,
            cancelled,
            FileVersions::filled(source.high_water_mark() as usize, FileVersion::INITIAL),
            true,
        )
    }

    fn from_parts(
        files: Arc<FileInventory>,
        fingerprints: Arc<Vec<Option<[u8; 32]>>>,
        base_configuration: [u8; 32],
        version: FileVersion,
    ) -> Self {
        let _ = files.scalar_index();
        let len = u32::try_from(files.files.len()).expect("file inventory length checked at build");
        let mut active = RoaringBitmap::new();
        active.insert_range(0..len);
        Self {
            base_files: files,
            base_fingerprints: fingerprints,
            construction_complete: true,
            base_configuration,
            changed: Arc::new(vec![Arc::new(HashMap::new()); SHARDS]),
            changed_paths: Arc::new(vec![Arc::new(HashMap::new()); SHARDS]),
            changed_relative_paths: Arc::new(BTreeMap::new()),
            changed_scalar: Arc::new(vec![Arc::new(FileScalarIndex::default()); SCALAR_SHARDS]),
            scalar_cache: Arc::new(std::sync::Mutex::new(BTreeMap::new())),
            active: Arc::new(active.clone()),
            scalar_base_live: Arc::new(active),
            len,
            versions: FileVersions::filled(len as usize, version),
        }
    }

    #[cfg(test)]
    pub(crate) fn allocated_files(&self) -> usize {
        self.len as usize
    }

    pub(crate) fn construction_complete(&self) -> bool {
        self.construction_complete
    }

    pub(crate) fn set_construction_complete(&mut self, complete: bool) {
        self.construction_complete = complete;
    }

    /// Rebind an otherwise unchanged discovered inventory to final metadata
    /// (global inference and captured fingerprints) without copying paths.
    fn rebase(&mut self, base: Box<dyn FileCatalogBase>) -> StorageResult<()> {
        if self.base_files.files.len() != base.high_water_mark() as usize
            || base.configuration() != self.base_configuration
            || base.root() != self.base_files.root()
        {
            return Err(StorageError::invalid(
                "rebase file catalog",
                "discovered files or configuration changed",
            ));
        }
        let (files, fingerprints) = if (&*base as &dyn std::any::Any).is::<DiscoveredFileRows>() {
            let rows = (base as Box<dyn std::any::Any>)
                .downcast::<DiscoveredFileRows>()
                .expect("checked final discovery rows");
            let files = self
                .base_files
                .rebased_entries(rows.files)
                .map_err(|error| StorageError::invalid("rebase file catalog", error))?;
            (Arc::new(files), Arc::new(rows.fingerprints))
        } else {
            native_base(base.as_ref())?
        };
        if !self.construction_complete {
            // Initial diagnostic overlays precede final digest assembly. Give
            // them the same captured fingerprint as unchanged base entries.
            let shards = Arc::make_mut(&mut self.changed);
            for shard in shards.iter_mut().filter(|shard| !shard.is_empty()) {
                for (id, changed) in Arc::make_mut(shard).iter_mut() {
                    changed.fingerprint = fingerprints.get(id.0 as usize).copied().flatten();
                    changed.configuration = changed.fingerprint.map(|_| self.base_configuration);
                }
            }
        }
        self.base_files = files;
        self.base_fingerprints = fingerprints;
        self.scalar_cache = Arc::new(std::sync::Mutex::new(BTreeMap::new()));
        Ok(())
    }

    pub(crate) fn active_files(&self) -> u64 {
        self.active.len()
    }

    pub(crate) fn scalar_delta_heap_bytes(&self) -> usize {
        self.changed_scalar
            .iter()
            .map(|shard| shard.estimated_heap_bytes())
            .sum()
    }

    fn scalar_ids(
        &self,
        limit: usize,
        mut select: impl FnMut(
            &FileScalarIndex,
            &mut Vec<FileId>,
            usize,
            Option<&RoaringBitmap>,
        ) -> bool,
    ) -> Option<Arc<[FileId]>> {
        let mut ids = Vec::new();
        let mask = (self.scalar_base_live.len() != self.base_files.files.len() as u64)
            .then_some(self.scalar_base_live.as_ref());
        if !select(self.base_files.scalar_index(), &mut ids, limit, mask) {
            return None;
        }
        for shard in self.changed_scalar.iter() {
            if !select(shard, &mut ids, limit, None) {
                return None;
            }
        }
        ids.sort_unstable();
        Some(ids.into())
    }

    fn cached_scalar_ids(
        &self,
        key: ScalarLookup,
        limit: usize,
        select: impl FnMut(&FileScalarIndex, &mut Vec<FileId>, usize, Option<&RoaringBitmap>) -> bool,
    ) -> Option<Arc<[FileId]>> {
        if limit > 4096 {
            return self.scalar_ids(limit, select);
        }
        let mut cache = self
            .scalar_cache
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if let Some(cached) = cache.get(&key) {
            return cached.clone();
        }
        let ids = self.scalar_ids(limit, select);
        if cache.len() == SCALAR_CACHE_LIMIT {
            cache.clear();
        }
        cache.insert(key, ids.clone());
        ids
    }

    pub(crate) fn size_ids(
        &self,
        operator: Operator,
        literal: &Number,
        limit: usize,
    ) -> Option<Arc<[FileId]>> {
        self.cached_scalar_ids(
            ScalarLookup::Size(operator_key(operator)?, literal.clone(), limit),
            limit,
            |index, ids, limit, mask| index.size_ids(operator, literal, ids, limit, mask),
        )
    }

    pub(crate) fn modified_ids(
        &self,
        operator: Operator,
        literal: i128,
        limit: usize,
    ) -> Option<Arc<[FileId]>> {
        self.cached_scalar_ids(
            ScalarLookup::Modified(operator_key(operator)?, literal, limit),
            limit,
            |index, ids, limit, mask| index.modified_ids(operator, literal, ids, limit, mask),
        )
    }

    pub(crate) fn created_ids(
        &self,
        operator: Operator,
        literal: i128,
        limit: usize,
    ) -> Option<Arc<[FileId]>> {
        self.cached_scalar_ids(
            ScalarLookup::Created(operator_key(operator)?, literal, limit),
            limit,
            |index, ids, limit, mask| index.created_ids(operator, literal, ids, limit, mask),
        )
    }

    pub(crate) fn unknown_created_ids(&self, limit: usize) -> Option<Arc<[FileId]>> {
        self.cached_scalar_ids(
            ScalarLookup::UnknownCreated(limit),
            limit,
            |index, ids, limit, mask| index.unknown_created_ids(ids, limit, mask),
        )
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
            .or_else(|| self.base_files.files.get(file.0 as usize))
    }

    pub(crate) fn fingerprint(&self, file: FileId) -> Option<[u8; 32]> {
        if !self.is_active(file) {
            return None;
        }
        self.changed[shard(&file)]
            .get(&file)
            .map(|changed| changed.fingerprint)
            .unwrap_or_else(|| {
                self.base_fingerprints
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
        self.entry_for_path(path).map(|entry| entry.id)
    }

    fn entry_for_path(&self, path: &Path) -> Option<&FileEntry> {
        // When all live paths still belong to the base, metadata edits may
        // remain but the path lookup can start in the base index.
        let id = if self.changed_relative_paths.is_empty() {
            self.base_files.id_for(path)
        } else {
            self.changed_paths[shard(&path)]
                .get(path)
                .copied()
                .or_else(|| self.base_files.id_for(path))
        }?;
        self.entry(id)
            .filter(|entry| entry.relative_path.as_path() == path)
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

    #[cfg(test)]
    pub(crate) fn relative_path_ids_in_bounds(
        &self,
        bounds: (std::ops::Bound<Arc<str>>, std::ops::Bound<Arc<str>>),
    ) -> Vec<FileId> {
        self.relative_path_ids_in_bounds_up_to(bounds, usize::MAX)
            .expect("an index cannot contain usize::MAX paths")
    }

    #[cfg(test)]
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
        bounds: FilePathBounds,
        limit: usize,
    ) -> Option<Vec<FileId>> {
        if empty_path_bounds(&bounds) {
            return Some(Vec::new());
        }
        let ids = self
            .live_path_ids(bounds)
            .take(limit.saturating_add(1))
            .collect::<Vec<_>>();
        (ids.len() <= limit).then_some(ids)
    }

    fn live_path_ids(&self, bounds: FilePathBounds) -> impl Iterator<Item = FileId> + '_ {
        let original = self
            .base_files
            .relative_paths
            .range(bounds.clone())
            .filter_map(|(_, id)| {
                (self.is_active(*id)
                    && self.changed[shard(id)].get(id).is_none_or(|changed| {
                        self.base_files
                            .files
                            .get(id.0 as usize)
                            .is_some_and(|baseline| {
                                changed.entry.relative_path == baseline.relative_path
                            })
                    }))
                .then_some(*id)
            });
        let changed = self
            .changed_relative_paths
            .range(path_bounds_with_ids(&bounds))
            .filter_map(|((key, id), _)| {
                (self.is_active(*id)
                    && self.changed[shard(id)]
                        .get(id)
                        .is_some_and(|changed| changed.entry.relative_path.query_key() == *key))
                .then_some(*id)
            });
        original.chain(changed)
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

    pub(crate) fn retired(&self, file: FileId) -> Result<Self, String> {
        let entry = self.entry(file).ok_or("unknown active file")?;
        let mut next = self.clone();
        next.scalar_cache = Arc::new(std::sync::Mutex::new(BTreeMap::new()));
        if self.scalar_base_live.contains(file.0) {
            Arc::make_mut(&mut next.scalar_base_live).remove(file.0);
        } else {
            let scalar = Arc::make_mut(&mut next.changed_scalar);
            Arc::make_mut(&mut scalar[file.0 as usize % SCALAR_SHARDS]).remove(entry);
        }
        if self.changed[shard(&file)].contains_key(&file) {
            let changed = Arc::make_mut(&mut next.changed);
            Arc::make_mut(&mut changed[shard(&file)]).remove(&file);
        }
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
        next.scalar_cache = Arc::new(std::sync::Mutex::new(BTreeMap::new()));
        let id = entry.id;
        let scalar = Arc::make_mut(&mut next.changed_scalar);
        Arc::make_mut(&mut scalar[id.0 as usize % SCALAR_SHARDS]).insert(&entry);
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
        next.versions = next.versions.appended()?;
        Ok(next)
    }

    pub(crate) fn replaced(
        &self,
        entry: FileEntry,
        fingerprint: Option<[u8; 32]>,
        configuration: Option<[u8; 32]>,
    ) -> Result<Self, String> {
        let id = entry.id;
        let previous = self.entry(id).ok_or("unknown active file")?;
        let old_path = previous.relative_path.clone();
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
        if !same_scalars(previous, &entry) {
            next.scalar_cache = Arc::new(std::sync::Mutex::new(BTreeMap::new()));
            let scalar = Arc::make_mut(&mut next.changed_scalar);
            let scalar = Arc::make_mut(&mut scalar[id.0 as usize % SCALAR_SHARDS]);
            if self.scalar_base_live.contains(id.0) {
                Arc::make_mut(&mut next.scalar_base_live).remove(id.0);
            } else {
                scalar.remove(previous);
            }
            if self
                .base_files
                .files
                .get(id.0 as usize)
                .is_some_and(|base| same_scalars(base, &entry))
            {
                Arc::make_mut(&mut next.scalar_base_live).insert(id.0);
            } else {
                scalar.insert(&entry);
            }
        }
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
            .base_files
            .files
            .get(id.0 as usize)
            .is_some_and(|baseline| baseline.relative_path == new_path);
        if !matches_base {
            ordered.insert((new_path.query_key(), id), ());
        }
        Ok(next)
    }
}

impl FileCatalogRevision for FileCatalog {
    fn construction_complete(&self) -> bool {
        self.construction_complete()
    }

    fn version(&self, id: FileId) -> StorageResult<Option<FileVersion>> {
        Ok(self.versions.get(id))
    }

    fn read_versions(
        &self,
        start: FileId,
        limit: BatchLimit,
    ) -> StorageResult<(Cow<'_, [FileVersion]>, BatchStatus)> {
        self.versions.read_batch(start, limit)
    }

    fn base_heap_bytes(&self) -> (usize, usize) {
        self.base_files.estimated_heap_bytes()
    }

    fn base_storage_diagnostic(&self) -> crate::inventory::FileStorageDiagnostic {
        self.base_files.storage_diagnostic()
    }

    fn estimates_delta_bytes(&self) -> usize {
        self.scalar_delta_heap_bytes()
    }

    fn fingerprint(&self, id: FileId) -> StorageResult<Option<[u8; 32]>> {
        Ok(self.fingerprint(id))
    }

    fn configuration(&self, id: FileId) -> StorageResult<Option<[u8; 32]>> {
        Ok(self.configuration(id))
    }

    fn edit(&self) -> StorageResult<Box<dyn FileCatalogEditor>> {
        Ok(Box::new(self.clone()))
    }
}

impl FileCatalogEditor for FileCatalog {
    fn apply(&mut self, delta: FileCatalogDelta) -> StorageResult<()> {
        match delta {
            FileCatalogDelta::Append {
                entry,
                fingerprint,
                configuration,
            } => {
                *self = self
                    .appended(entry, fingerprint, configuration)
                    .map_err(|error| StorageError::invalid("append file metadata", error))?;
            }
            FileCatalogDelta::Replace {
                entry,
                fingerprint,
                configuration,
            } => {
                *self = self
                    .replaced(entry, fingerprint, configuration)
                    .map_err(|error| StorageError::invalid("replace file metadata", error))?;
            }
            FileCatalogDelta::Retire(file) => {
                *self = self
                    .retired(file)
                    .map_err(|error| StorageError::invalid("retire file metadata", error))?;
            }
            FileCatalogDelta::AdvanceVersion(id) => {
                self.versions = self
                    .versions
                    .advanced(id)
                    .map_err(StorageError::ResourceLimit)?
                    .0;
            }
            FileCatalogDelta::ResetVersions(version) => {
                self.versions = FileVersions::filled(self.len as usize, version);
            }
            FileCatalogDelta::Rebase(base) => self.rebase(base)?,
            FileCatalogDelta::ConstructionComplete(complete) => {
                self.set_construction_complete(complete)
            }
        }
        Ok(())
    }

    fn seal(self: Box<Self>) -> StorageResult<FileCatalogHandle> {
        Ok(Arc::from(self as Box<dyn FileCatalogRevision>))
    }
}

impl FileCatalogReader for FileCatalog {
    fn repository_root(&self) -> Option<&Path> {
        self.base_files.root()
    }

    fn file(&self, id: FileId) -> StorageResult<Option<Cow<'_, FileEntry>>> {
        Ok(self.entry(id).map(Cow::Borrowed))
    }

    fn id_for_path(&self, path: &Path) -> StorageResult<Option<FileId>> {
        Ok(self.id_for(path))
    }

    fn file_by_path(&self, path: &Path) -> StorageResult<Option<Cow<'_, FileEntry>>> {
        Ok(self.entry_for_path(path).map(Cow::Borrowed))
    }

    fn path_candidates(
        &self,
        bounds: FilePathBounds,
        limit: usize,
    ) -> StorageResult<Option<Vec<FileId>>> {
        Ok(self.relative_path_ids_in_bounds_up_to(bounds, limit))
    }

    fn visit_paths(
        &self,
        bounds: FilePathBounds,
        visit: &mut dyn FnMut(&[FileId]) -> StorageResult<()>,
    ) -> StorageResult<()> {
        if empty_path_bounds(&bounds) {
            return Ok(());
        }
        let mut batch = smallvec::SmallVec::<[_; 256]>::new();
        for id in self.live_path_ids(bounds) {
            batch.push(id);
            if batch.len() == batch.inline_size() {
                visit(&batch)?;
                batch.clear();
            }
        }
        if !batch.is_empty() {
            visit(&batch)?;
        }
        Ok(())
    }

    fn scalar_candidates(
        &self,
        predicate: FileScalarPredicate<'_>,
        limit: usize,
    ) -> StorageResult<Option<Arc<[FileId]>>> {
        Ok(match predicate {
            FileScalarPredicate::Size(operator, literal) => self.size_ids(operator, literal, limit),
            FileScalarPredicate::Modified(operator, literal) => {
                self.modified_ids(operator, literal, limit)
            }
            FileScalarPredicate::Created(operator, literal) => {
                self.created_ids(operator, literal, limit)
            }
            FileScalarPredicate::UnknownCreated => self.unknown_created_ids(limit),
        })
    }
}

impl FileCatalogSource for FileCatalog {
    fn high_water_mark(&self) -> u32 {
        self.len
    }

    fn live_count(&self) -> u64 {
        self.active_files()
    }

    fn read_batch<'a>(
        &'a self,
        after: Option<FileId>,
        limit: BatchLimit,
        output: &mut Vec<FileTransferRow<'a>>,
    ) -> StorageResult<BatchStatus> {
        const OP: &str = "read file catalog";
        let start = after.map_or(0, |file| file.0.saturating_add(1));
        let mut bytes = 0usize;
        for number in self.active.range(start..) {
            let id = FileId(number);
            let Some(entry) = self.entry(id) else {
                return Err(StorageError::invalid(
                    OP,
                    format!("missing active file ID {number}"),
                ));
            };
            let row = FileTransferRow {
                entry: Cow::Borrowed(entry),
                fingerprint: self.fingerprint(id),
                configuration: self.configuration(id),
            };
            let row_bytes = row.bytes();
            if !limit.accepts(OP, output.len(), bytes, row_bytes)? {
                return Ok(BatchStatus::More);
            }
            bytes += row_bytes;
            output.push(row);
        }
        Ok(BatchStatus::Done)
    }
}

#[cfg(test)]
#[path = "file_catalog/tests.rs"]
mod tests;
