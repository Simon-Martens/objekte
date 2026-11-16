//! Immutable archived file metadata and path catalog shards.
//!
//! Published lookups are backed by rkyv maps and vectors. `FileEntry` values
//! returned by compatibility APIs come from a synchronized per-shard cache;
//! they never reconstruct lookup maps.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::ops::Bound;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::path::PathBuf as TestPathBuf;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{Arc, LazyLock, OnceLock};

use rayon::prelude::*;
use rayon::ThreadPool;
use rkyv::{Archive, Serialize as ArchiveSerialize};
use roaring::RoaringBitmap;

use crate::archive_storage::ValidatedArchive;
use crate::archived_path::StoredPath;
use crate::inventory::{
    ByteRange, FileEntry, FileInventory, FileStatus, FileVersion, FileVersions,
    SourceDiagnostic, SourceRevision,
    empty_path_bounds, relative_path_bounds,
};
use crate::lifecycle_scan::FileStamp;
use crate::model::{CollectionId, FileId, RecordSlot};
use crate::native_segment::SegmentId;
use crate::query::Operator;
use crate::repository::Repository;
use crate::source::DiagnosticCategory;

const SHARDS: usize = 1024;
const FILE_SHARD_WIDTH: u32 = 1024;

pub(crate) fn shard<T: Hash>(value: &T) -> usize {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    (hasher.finish() as usize) % SHARDS
}

fn file_shard_number(file: FileId) -> usize {
    (file.0 / FILE_SHARD_WIDTH) as usize
}

fn valid_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_)))
}

fn category_code(category: DiagnosticCategory) -> u8 {
    match category {
        DiagnosticCategory::Framing => 0,
        DiagnosticCategory::Encoding => 1,
        DiagnosticCategory::Syntax => 2,
        DiagnosticCategory::LogicalValidation => 3,
        DiagnosticCategory::ResourceLimit => 4,
        DiagnosticCategory::Io => 5,
    }
}

fn category_from_code(code: u8) -> Option<DiagnosticCategory> {
    Some(match code {
        0 => DiagnosticCategory::Framing,
        1 => DiagnosticCategory::Encoding,
        2 => DiagnosticCategory::Syntax,
        3 => DiagnosticCategory::LogicalValidation,
        4 => DiagnosticCategory::ResourceLimit,
        5 => DiagnosticCategory::Io,
        _ => return None,
    })
}

#[derive(Archive, ArchiveSerialize)]
struct StoredCatalogDiagnostic {
    file: u32,
    collection: u32,
    source_id: String,
    source_version: u32,
    record: Option<u32>,
    range_start: Option<u64>,
    range_end: Option<u64>,
    category: u8,
    code: String,
    field_path: Option<Vec<String>>,
    byte_offset: Option<u64>,
    line: Option<u64>,
    column: Option<u64>,
    message: String,
}

impl StoredCatalogDiagnostic {
    fn from_diagnostic(diagnostic: &SourceDiagnostic) -> Self {
        Self {
            file: diagnostic.file.0,
            collection: diagnostic.collection.0,
            source_id: diagnostic.source_id.clone(),
            source_version: diagnostic.source_version,
            record: diagnostic.record.map(|slot| slot.0),
            range_start: diagnostic.range.map(|range| range.start),
            range_end: diagnostic.range.map(|range| range.end),
            category: category_code(diagnostic.category),
            code: diagnostic.code.clone(),
            field_path: diagnostic.field_path.clone(),
            byte_offset: diagnostic.byte_offset,
            line: diagnostic.line,
            column: diagnostic.column,
            message: diagnostic.message.clone(),
        }
    }

    fn to_diagnostic(&self) -> Result<SourceDiagnostic, String> {
        let range = match (self.range_start, self.range_end) {
            (Some(start), Some(end)) => Some(ByteRange { start, end }),
            (None, None) => None,
            _ => return Err("archived diagnostic range is incomplete".into()),
        };
        Ok(SourceDiagnostic {
            file: FileId(self.file),
            collection: CollectionId(self.collection),
            source_id: self.source_id.clone(),
            source_version: self.source_version,
            record: self.record.map(RecordSlot),
            range,
            category: category_from_code(self.category)
                .ok_or("archived diagnostic category is invalid")?,
            code: self.code.clone(),
            field_path: self.field_path.clone(),
            byte_offset: self.byte_offset,
            line: self.line,
            column: self.column,
            message: self.message.clone(),
        })
    }

    fn from_archived(diagnostic: &rkyv::Archived<Self>) -> Self {
        Self {
            file: diagnostic.file.to_native(),
            collection: diagnostic.collection.to_native(),
            source_id: diagnostic.source_id.as_str().into(),
            source_version: diagnostic.source_version.to_native(),
            record: diagnostic.record.as_ref().map(|slot| slot.to_native()),
            range_start: diagnostic
                .range_start
                .as_ref()
                .map(|value| value.to_native()),
            range_end: diagnostic
                .range_end
                .as_ref()
                .map(|value| value.to_native()),
            category: diagnostic.category,
            code: diagnostic.code.as_str().into(),
            field_path: diagnostic.field_path.as_ref().map(|path| {
                path.iter()
                    .map(|part| part.as_str().to_owned())
                    .collect()
            }),
            byte_offset: diagnostic
                .byte_offset
                .as_ref()
                .map(|value| value.to_native()),
            line: diagnostic.line.as_ref().map(|value| value.to_native()),
            column: diagnostic
                .column
                .as_ref()
                .map(|value| value.to_native()),
            message: diagnostic.message.as_str().into(),
        }
    }
}

#[derive(Archive, ArchiveSerialize)]
struct StoredFileRow {
    id: u32,
    path: StoredPath,
    modified_ns: i128,
    size: u64,
    created_ns: Option<i128>,
    collections: Vec<u32>,
    status: u8,
    diagnostics: Vec<StoredCatalogDiagnostic>,
    omitted_diagnostics: u32,
    fingerprint: Option<Vec<u8>>,
    configuration: Option<Vec<u8>>,
}

impl StoredFileRow {
    fn from_entry(
        entry: &FileEntry,
        fingerprint: Option<[u8; 32]>,
        configuration: Option<[u8; 32]>,
    ) -> Self {
        Self {
            id: entry.id.0,
            path: StoredPath::from_shared(&entry.relative_path),
            modified_ns: entry.revision.modified_ns,
            size: entry.revision.size,
            created_ns: entry.created_ns,
            collections: entry
                .collections
                .iter()
                .map(|collection| collection.0)
                .collect(),
            status: entry.status.code(),
            diagnostics: entry
                .diagnostics
                .iter()
                .map(StoredCatalogDiagnostic::from_diagnostic)
                .collect(),
            omitted_diagnostics: entry.omitted_diagnostics,
            fingerprint: fingerprint.map(Vec::from),
            configuration: configuration.map(Vec::from),
        }
    }

    fn validate_stored(&self) -> Result<(), String> {
        if !self.path.is_valid_relative()
            || self.collections.is_empty()
            || self
                .collections
                .iter()
                .copied()
                .collect::<HashSet<_>>()
                .len()
                != self.collections.len()
            || self.status > FileStatus::Unreadable.code()
            || self.diagnostics.len() > crate::inventory::MAX_DIAGNOSTICS_PER_FILE
            || self
                .fingerprint
                .as_ref()
                .is_some_and(|value| value.len() != 32)
            || self
                .configuration
                .as_ref()
                .is_some_and(|value| value.len() != 32)
            || self.diagnostics.iter().any(|diagnostic| {
                diagnostic.file != self.id
                    || !self.collections.contains(&diagnostic.collection)
                    || category_from_code(diagnostic.category).is_none()
                    || matches!(
                        (diagnostic.range_start, diagnostic.range_end),
                        (Some(_), None) | (None, Some(_))
                    )
            })
        {
            return Err("invalid stored file catalog row".into());
        }
        Ok(())
    }

    fn to_entry(&self) -> Result<FileEntry, String> {
        let status = match self.status {
            0 => FileStatus::Valid,
            1 => FileStatus::Broken,
            2 => FileStatus::Unreadable,
            _ => return Err("archived file status is invalid".into()),
        };
        let entry = FileEntry {
            id: FileId(self.id),
            relative_path: self.path.to_shared()?,
            revision: SourceRevision {
                modified_ns: self.modified_ns,
                size: self.size,
            },
            created_ns: self.created_ns,
            collections: self
                .collections
                .iter()
                .copied()
                .map(CollectionId)
                .collect(),
            status,
            diagnostics: self
                .diagnostics
                .iter()
                .map(StoredCatalogDiagnostic::to_diagnostic)
                .collect::<Result<_, _>>()?,
            omitted_diagnostics: self.omitted_diagnostics,
        };
        validate_entry(&entry, self.id as usize)?;
        Ok(entry)
    }

    fn from_archived(row: &rkyv::Archived<Self>) -> Self {
        Self {
            id: row.id.to_native(),
            path: StoredPath::from_archived(&row.path),
            modified_ns: row.modified_ns.to_native(),
            size: row.size.to_native(),
            created_ns: row.created_ns.as_ref().map(|value| value.to_native()),
            collections: row
                .collections
                .iter()
                .map(|collection| collection.to_native())
                .collect(),
            status: row.status,
            diagnostics: row
                .diagnostics
                .iter()
                .map(StoredCatalogDiagnostic::from_archived)
                .collect(),
            omitted_diagnostics: row.omitted_diagnostics.to_native(),
            fingerprint: row
                .fingerprint
                .as_ref()
                .map(|bytes| bytes.as_slice().to_vec()),
            configuration: row
                .configuration
                .as_ref()
                .map(|bytes| bytes.as_slice().to_vec()),
        }
    }
}

#[derive(Archive, ArchiveSerialize)]
struct CatalogMetadataArchive {
    id_lookup: HashMap<u32, u32>,
    rows: Vec<StoredFileRow>,
}

struct CatalogMetadataShard {
    archive: Arc<ValidatedArchive<CatalogMetadataArchive>>,
    cache: OnceLock<Vec<FileEntry>>,
}

#[derive(Archive, ArchiveSerialize)]
struct CatalogPathArchive {
    exact: HashMap<String, u32>,
    ordered: BTreeMap<String, u32>,
}

struct CatalogPathShard {
    archive: Arc<ValidatedArchive<CatalogPathArchive>>,
}

#[derive(Clone)]
pub(crate) struct PathDirectory {
    shards: Arc<Vec<Arc<CatalogPathShard>>>,
}

impl std::fmt::Debug for PathDirectory {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PathDirectory")
            .field("shards", &self.shards.len())
            .finish()
    }
}

impl PathDirectory {
    pub(crate) fn id_for(&self, path: &Path) -> Option<FileId> {
        let query = crate::path_repr::query_path(path);
        self.shards[shard(&query)]
            .archive
            .root()
            .exact
            .get::<str>(query.as_str())
            .copied()
            .map(|id| FileId(id.to_native()))
    }

    pub(crate) fn ids_in_bounds_up_to(
        &self,
        bounds: (Bound<Arc<str>>, Bound<Arc<str>>),
        limit: usize,
    ) -> Option<Vec<FileId>> {
        if empty_path_bounds(&bounds) {
            return Some(Vec::new());
        }
        let bounds = string_bounds(&bounds);
        let mut rows = Vec::new();
        let mut scanned = 0usize;
        for path_shard in self.shards.iter() {
            for (query, id) in path_shard
                .archive
                .root()
                .ordered
                .range_with(bounds.clone(), |query, archived| {
                    query.as_str().cmp(archived.as_str())
                })
            {
                if scanned == limit {
                    return None;
                }
                scanned += 1;
                rows.push((query.as_str(), FileId(id.to_native())));
            }
        }
        crate::storage_usage::StorageWorkCounters::update(|counters| {
            counters.path_entries_examined = counters
                .path_entries_examined
                .saturating_add(scanned as u64);
        });
        rows.sort_by(|left, right| left.0.cmp(right.0).then_with(|| left.1.cmp(&right.1)));
        Some(rows.into_iter().map(|(_, id)| id).collect())
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct OwnershipRoute {
    pub collection: CollectionId,
    pub segment: SegmentId,
    pub file_row_ordinal: u32,
}

#[derive(Archive, ArchiveSerialize)]
struct StoredOwnershipRoute {
    collection: u32,
    segment: u64,
    file_row_ordinal: u32,
}

#[derive(Archive, ArchiveSerialize)]
struct StoredOwnershipRow {
    file: u32,
    version: u64,
    routes: Vec<StoredOwnershipRoute>,
}

#[derive(Archive, ArchiveSerialize)]
struct OwnershipArchive {
    lookup: HashMap<u32, u32>,
    rows: Vec<StoredOwnershipRow>,
}

struct OwnershipShard {
    archive: Arc<ValidatedArchive<OwnershipArchive>>,
}

/// One published view's active file table. Previous views retain their own
/// archived shards and liveness bitmap after a new root is published.
#[derive(Clone)]
pub(crate) struct FileCatalog {
    root: PathBuf,
    construction_complete: bool,
    inventory: Arc<FileInventory>,
    metadata: Arc<Vec<Arc<CatalogMetadataShard>>>,
    paths: Arc<Vec<Arc<CatalogPathShard>>>,
    ownership: Arc<Vec<Arc<OwnershipShard>>>,
    active: Arc<RoaringBitmap>,
    len: u32,
}

fn validate_entry(entry: &FileEntry, expected_id: usize) -> Result<(), String> {
    if entry.id.0 as usize != expected_id
        || !valid_relative_path(&entry.relative_path)
        || entry.collections.is_empty()
        || entry
            .collections
            .iter()
            .copied()
            .collect::<HashSet<_>>()
            .len()
            != entry.collections.len()
        || entry.diagnostics.len() > crate::inventory::MAX_DIAGNOSTICS_PER_FILE
        || entry.diagnostics.iter().any(|diagnostic| {
            diagnostic.file != entry.id || !entry.collections.contains(&diagnostic.collection)
        })
    {
        return Err("invalid file catalog entry".into());
    }
    Ok(())
}

fn archived_diagnostic_matches(
    archived: &rkyv::Archived<StoredCatalogDiagnostic>,
    current: &SourceDiagnostic,
) -> bool {
    archived.file.to_native() == current.file.0
        && archived.collection.to_native() == current.collection.0
        && archived.source_id.as_str() == current.source_id
        && archived.source_version.to_native() == current.source_version
        && archived.record.as_ref().map(|slot| slot.to_native()) == current.record.map(|slot| slot.0)
        && archived.range_start.as_ref().map(|value| value.to_native())
            == current.range.map(|range| range.start)
        && archived.range_end.as_ref().map(|value| value.to_native())
            == current.range.map(|range| range.end)
        && archived.category == category_code(current.category)
        && archived.code.as_str() == current.code
        && archived.field_path.as_ref().map_or(current.field_path.is_none(), |path| {
            current.field_path.as_ref().is_some_and(|current| {
                path.len() == current.len()
                    && path
                        .iter()
                        .zip(current)
                        .all(|(archived, current)| archived.as_str() == current)
            })
        })
        && archived.byte_offset.as_ref().map(|value| value.to_native())
            == current.byte_offset
        && archived.line.as_ref().map(|value| value.to_native()) == current.line
        && archived.column.as_ref().map(|value| value.to_native()) == current.column
        && archived.message.as_str() == current.message
}

fn archived_catalog_row_matches(
    archived: &rkyv::Archived<StoredFileRow>,
    current: &FileEntry,
    fingerprint: Option<[u8; 32]>,
    configuration: Option<[u8; 32]>,
) -> bool {
    archived.id.to_native() == current.id.0
        && crate::archived_path::matches_native(&archived.path, current.relative_path.as_path())
        && archived.modified_ns.to_native() == current.revision.modified_ns
        && archived.size.to_native() == current.revision.size
        && archived.created_ns.as_ref().map(|value| value.to_native()) == current.created_ns
        && archived.collections.len() == current.collections.len()
        && archived
            .collections
            .iter()
            .zip(&current.collections)
            .all(|(archived, current)| archived.to_native() == current.0)
        && archived.status == current.status.code()
        && archived.diagnostics.len() == current.diagnostics.len()
        && archived
            .diagnostics
            .iter()
            .zip(&current.diagnostics)
            .all(|(archived, current)| archived_diagnostic_matches(archived, current))
        && archived.omitted_diagnostics.to_native() == current.omitted_diagnostics
        && archived.fingerprint.as_ref().map(|value| value.as_slice())
            == fingerprint.as_ref().map(|value| value.as_slice())
        && archived.configuration.as_ref().map(|value| value.as_slice())
            == configuration.as_ref().map(|value| value.as_slice())
}

fn build_metadata_shard(
    rows: impl IntoIterator<Item = StoredFileRow>,
) -> Result<Arc<CatalogMetadataShard>, String> {
    crate::storage_usage::StorageWorkCounters::update(|counters| {
        counters.metadata_shards_rebuilt = counters.metadata_shards_rebuilt.saturating_add(1);
    });
    let mut rows = rows.into_iter().collect::<Vec<_>>();
    rows.sort_by_key(|row| row.id);
    let mut id_lookup = HashMap::with_capacity(rows.len());
    for (ordinal, row) in rows.iter().enumerate() {
        if id_lookup
            .insert(row.id, u32::try_from(ordinal).map_err(|_| "catalog shard overflow")?)
            .is_some()
        {
            return Err("duplicate file ID in catalog shard".into());
        }
        row.validate_stored()?;
    }
    let archive = ValidatedArchive::freeze(&CatalogMetadataArchive { id_lookup, rows })?;
    let shard = Arc::new(CatalogMetadataShard {
        archive,
        cache: OnceLock::new(),
    });
    for row in shard.archive.root().rows.iter() {
        let id = row.id.to_native();
        let key = rkyv::rend::u32_le::from_native(id);
        let ordinal = shard
            .archive
            .root()
            .id_lookup
            .get(&key)
            .ok_or("archived catalog ID lookup missing row")?;
        if ordinal.to_native() as usize >= shard.archive.root().rows.len() {
            return Err("archived catalog row ordinal out of bounds".into());
        }
    }
    Ok(shard)
}

static EMPTY_METADATA_SHARD: LazyLock<Arc<CatalogMetadataShard>> = LazyLock::new(|| {
    build_metadata_shard(std::iter::empty()).expect("empty catalog shard serializes")
});
static EMPTY_PATH_SHARD: LazyLock<Arc<CatalogPathShard>> = LazyLock::new(|| {
    Arc::new(CatalogPathShard {
        archive: ValidatedArchive::freeze(&CatalogPathArchive {
            exact: HashMap::new(),
            ordered: BTreeMap::new(),
        })
        .expect("empty path shard serializes"),
    })
});
static EMPTY_OWNERSHIP_SHARD: LazyLock<Arc<OwnershipShard>> = LazyLock::new(|| {
    build_ownership_shard(std::iter::empty()).expect("empty ownership shard serializes")
});

fn empty_metadata_shard() -> Arc<CatalogMetadataShard> {
    Arc::clone(&EMPTY_METADATA_SHARD)
}

fn empty_path_shard() -> Arc<CatalogPathShard> {
    Arc::clone(&EMPTY_PATH_SHARD)
}

fn empty_ownership_shard() -> Arc<OwnershipShard> {
    Arc::clone(&EMPTY_OWNERSHIP_SHARD)
}

fn build_path_shard(
    entries: impl IntoIterator<Item = (String, FileId)>,
) -> Result<Arc<CatalogPathShard>, String> {
    crate::storage_usage::StorageWorkCounters::update(|counters| {
        counters.path_shards_rebuilt = counters.path_shards_rebuilt.saturating_add(1);
    });
    let mut exact = HashMap::new();
    let mut ordered = BTreeMap::new();
    for (query, id) in entries {
        if exact.insert(query.clone(), id.0).is_some() {
            return Err("duplicate exact path in catalog shard".into());
        }
        ordered.insert(query, id.0);
    }
    Ok(Arc::new(CatalogPathShard {
        archive: ValidatedArchive::freeze(&CatalogPathArchive { exact, ordered })?,
    }))
}

fn build_ownership_shard(
    rows: impl IntoIterator<Item = StoredOwnershipRow>,
) -> Result<Arc<OwnershipShard>, String> {
    crate::storage_usage::StorageWorkCounters::update(|counters| {
        counters.ownership_shards_rebuilt =
            counters.ownership_shards_rebuilt.saturating_add(1);
    });
    let mut rows = rows.into_iter().collect::<Vec<_>>();
    rows.sort_by_key(|row| row.file);
    let mut lookup = HashMap::with_capacity(rows.len());
    for (ordinal, row) in rows.iter_mut().enumerate() {
        if row.routes.is_empty() {
            return Err("ownership row has no active route".into());
        }
        row.routes.sort_by_key(|route| {
            (route.collection, route.segment, route.file_row_ordinal)
        });
        row.routes.dedup_by_key(|route| {
            (route.collection, route.segment, route.file_row_ordinal)
        });
        if lookup
            .insert(
                row.file,
                u32::try_from(ordinal).map_err(|_| "ownership shard overflow")?,
            )
            .is_some()
        {
            return Err("duplicate ownership file row".into());
        }
    }
    Ok(Arc::new(OwnershipShard {
        archive: ValidatedArchive::freeze(&OwnershipArchive { lookup, rows })?,
    }))
}

impl OwnershipShard {
    fn row(&self, file: FileId) -> Option<&rkyv::Archived<StoredOwnershipRow>> {
        let key = rkyv::rend::u32_le::from_native(file.0);
        let ordinal = self.archive.root().lookup.get(&key)?.to_native() as usize;
        self.archive.root().rows.get(ordinal)
    }
}

fn archived_ownership_route(
    route: &rkyv::Archived<StoredOwnershipRoute>,
) -> OwnershipRoute {
    OwnershipRoute {
        collection: CollectionId(route.collection.to_native()),
        segment: SegmentId::new(route.segment.to_native()),
        file_row_ordinal: route.file_row_ordinal.to_native(),
    }
}

fn stored_ownership_route(route: OwnershipRoute) -> StoredOwnershipRoute {
    StoredOwnershipRoute {
        collection: route.collection.0,
        segment: route.segment.get(),
        file_row_ordinal: route.file_row_ordinal,
    }
}

impl CatalogMetadataShard {
    fn row_ordinal(&self, file: FileId) -> Option<usize> {
        let key = rkyv::rend::u32_le::from_native(file.0);
        self.archive
            .root()
            .id_lookup
            .get(&key)
            .map(|ordinal| ordinal.to_native() as usize)
    }

    fn archived_row(&self, file: FileId) -> Option<&rkyv::Archived<StoredFileRow>> {
        self.archive.root().rows.get(self.row_ordinal(file)?)
    }

    fn entries(&self) -> &[FileEntry] {
        self.cache.get_or_init(|| {
            crate::storage_usage::StorageWorkCounters::update(|counters| {
                counters.metadata_cache_initializations =
                    counters.metadata_cache_initializations.saturating_add(1);
                counters.catalog_row_cache_initializations = counters
                    .catalog_row_cache_initializations
                    .saturating_add(1);
            });
            self.archive
                .root()
                .rows
                .iter()
                .map(|row| {
                    let id = row.id.to_native();
                    let path = crate::archived_path::to_shared(&row.path)
                        .expect("validated archived path");
                    let status = match row.status {
                        0 => FileStatus::Valid,
                        1 => FileStatus::Broken,
                        2 => FileStatus::Unreadable,
                        _ => unreachable!("validated archived file status"),
                    };
                    FileEntry {
                        id: FileId(id),
                        relative_path: path,
                        revision: SourceRevision {
                            modified_ns: row.modified_ns.to_native(),
                            size: row.size.to_native(),
                        },
                        created_ns: row.created_ns.as_ref().map(|value| value.to_native()),
                        collections: row
                            .collections
                            .iter()
                            .map(|collection| CollectionId(collection.to_native()))
                            .collect(),
                        status,
                        diagnostics: row
                            .diagnostics
                            .iter()
                            .map(|diagnostic| {
                                let range = match (
                                    diagnostic.range_start.as_ref(),
                                    diagnostic.range_end.as_ref(),
                                ) {
                                    (Some(start), Some(end)) => Some(ByteRange {
                                        start: start.to_native(),
                                        end: end.to_native(),
                                    }),
                                    (None, None) => None,
                                    _ => unreachable!("validated diagnostic range"),
                                };
                                SourceDiagnostic {
                                    file: FileId(diagnostic.file.to_native()),
                                    collection: CollectionId(
                                        diagnostic.collection.to_native(),
                                    ),
                                    source_id: diagnostic.source_id.as_str().into(),
                                    source_version: diagnostic.source_version.to_native(),
                                    record: diagnostic
                                        .record
                                        .as_ref()
                                        .map(|slot| RecordSlot(slot.to_native())),
                                    range,
                                    category: category_from_code(diagnostic.category)
                                        .expect("validated diagnostic category"),
                                    code: diagnostic.code.as_str().into(),
                                    field_path: diagnostic.field_path.as_ref().map(|path| {
                                        path.iter()
                                            .map(|part| part.as_str().to_owned())
                                            .collect()
                                    }),
                                    byte_offset: diagnostic
                                        .byte_offset
                                        .as_ref()
                                        .map(|value| value.to_native()),
                                    line: diagnostic
                                        .line
                                        .as_ref()
                                        .map(|value| value.to_native()),
                                    column: diagnostic
                                        .column
                                        .as_ref()
                                        .map(|value| value.to_native()),
                                    message: diagnostic.message.as_str().into(),
                                }
                            })
                            .collect(),
                        omitted_diagnostics: row.omitted_diagnostics.to_native(),
                    }
                })
                .collect()
        })
    }
}

fn string_bounds(
    bounds: &(Bound<Arc<str>>, Bound<Arc<str>>),
) -> (Bound<String>, Bound<String>) {
    let map = |bound: &Bound<Arc<str>>| match bound {
        Bound::Unbounded => Bound::Unbounded,
        Bound::Included(value) => Bound::Included(value.to_string()),
        Bound::Excluded(value) => Bound::Excluded(value.to_string()),
    };
    (map(&bounds.0), map(&bounds.1))
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

    #[cfg(test)]
    pub(crate) fn from_repository(base: Arc<Repository>) -> Result<Self, String> {
        let configuration = Self::configuration_for(&base)?;
        Ok(Self::from_parts(
            Arc::clone(&base.files),
            base.base_fingerprints.clone(),
            configuration,
        ))
    }

    pub(crate) fn from_parts(
        files: Arc<FileInventory>,
        fingerprints: Vec<Option<[u8; 32]>>,
        configuration_identity: [u8; 32],
    ) -> Self {
        Self::from_parts_in_pool(files, fingerprints, configuration_identity, None)
    }

    pub(crate) fn from_parts_in_pool(
        files: Arc<FileInventory>,
        fingerprints: Vec<Option<[u8; 32]>>,
        configuration_identity: [u8; 32],
        pool: Option<&ThreadPool>,
    ) -> Self {
        let len = u32::try_from(files.files.len()).expect("file inventory length checked at build");
        let mut path_buckets = vec![Vec::new(); SHARDS];
        let mut grouped_rows =
            (0..=file_shard_number(FileId(len.saturating_sub(1))))
                .map(|_| Vec::new())
                .collect::<Vec<_>>();
        let mut active = RoaringBitmap::new();
        for entry in &files.files {
            active.insert(entry.id.0);
            let fingerprint = fingerprints.get(entry.id.0 as usize).copied().flatten();
            let configuration = fingerprint.map(|_| configuration_identity);
            grouped_rows[file_shard_number(entry.id)].push(StoredFileRow::from_entry(
                entry,
                fingerprint,
                configuration,
            ));
            let query = entry.relative_path.query_key().to_string();
            path_buckets[shard(&entry.relative_path.query_key())].push((query, entry.id));
        }
        let (metadata, paths) = match pool {
            Some(pool) => (
                pool.install(|| {
                    grouped_rows
                        .into_par_iter()
                        .map(|rows| {
                            if rows.is_empty() {
                                empty_metadata_shard()
                            } else {
                                build_metadata_shard(rows).expect(
                                    "file inventory entries pass catalog validation",
                                )
                            }
                        })
                        .collect::<Vec<_>>()
                }),
                pool.install(|| {
                    path_buckets
                        .into_par_iter()
                        .map(|entries| {
                            if entries.is_empty() {
                                empty_path_shard()
                            } else {
                                build_path_shard(entries)
                                    .expect("file inventory paths are unique")
                            }
                        })
                        .collect::<Vec<_>>()
                }),
            ),
            None => (
                grouped_rows
                    .into_iter()
                    .map(|rows| {
                        if rows.is_empty() {
                            empty_metadata_shard()
                        } else {
                            build_metadata_shard(rows)
                                .expect("file inventory entries pass catalog validation")
                        }
                    })
                    .collect(),
                path_buckets
                    .into_iter()
                    .map(|entries| {
                        if entries.is_empty() {
                            empty_path_shard()
                        } else {
                            build_path_shard(entries)
                                .expect("file inventory paths are unique")
                        }
                    })
                    .collect(),
            ),
        };
        Self {
            root: files.root.clone(),
            construction_complete: true,
            inventory: Arc::clone(&files),
            metadata: Arc::new(metadata),
            paths: Arc::new(paths),
            ownership: Arc::new(
                (0..=file_shard_number(FileId(len.saturating_sub(1))))
                    .map(|_| empty_ownership_shard())
                    .collect(),
            ),
            active: Arc::new(active),
            len,
        }
    }

    pub(crate) fn snapshot_slots(
        &self,
    ) -> Vec<Option<(FileEntry, Option<[u8; 32]>, Option<[u8; 32]>)>> {
        (0..self.len)
            .map(|number| {
                let id = FileId(number);
                self.entry(id)
                    .map(|entry| (entry.clone(), self.fingerprint(id), self.configuration(id)))
            })
            .collect()
    }

    pub(crate) fn restored(
        base: Arc<Repository>,
        slots: Vec<Option<(FileEntry, Option<[u8; 32]>, Option<[u8; 32]>)>>,
    ) -> Result<Self, String> {
        let _configuration = Self::configuration_for(&base)?;
        if slots.len() < base.files().files.len() {
            return Err("restored catalog is shorter than base inventory".into());
        }
        let len = u32::try_from(slots.len()).map_err(|_| "too many indexed files")?;
        let mut grouped_rows: Vec<Vec<StoredFileRow>> =
            (0..=file_shard_number(FileId(len)))
                .map(|_| Vec::new())
                .collect();
        let mut path_buckets = vec![Vec::new(); SHARDS];
        let mut active = RoaringBitmap::new();
        let mut paths = HashSet::new();
        for (number, slot) in slots.into_iter().enumerate() {
            let Some((entry, fingerprint, entry_configuration)) = slot else {
                continue;
            };
            validate_entry(&entry, number)?;
            if entry
                .collections
                .iter()
                .any(|collection| collection.0 as usize >= base.collections().len())
                || !paths.insert(entry.relative_path.clone())
            {
                return Err("invalid restored file catalog entry".into());
            }
            active.insert(entry.id.0);
            let query = entry.relative_path.query_key().to_string();
            path_buckets[shard(&entry.relative_path.query_key())].push((query, entry.id));
            grouped_rows[file_shard_number(entry.id)].push(StoredFileRow::from_entry(
                &entry,
                fingerprint,
                entry_configuration,
            ));
        }
        let metadata = grouped_rows
            .into_iter()
            .map(build_metadata_shard)
            .collect::<Result<Vec<_>, _>>()?;
        let paths = path_buckets
            .into_iter()
            .map(|entries| {
                if entries.is_empty() {
                    Ok(empty_path_shard())
                } else {
                    build_path_shard(entries)
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        let catalog = Self {
            root: base.files().root.clone(),
            construction_complete: true,
            inventory: Arc::clone(&base.files),
            metadata: Arc::new(metadata),
            paths: Arc::new(paths),
            ownership: Arc::new(
                (0..=file_shard_number(FileId(len.saturating_sub(1))))
                    .map(|_| empty_ownership_shard())
                    .collect(),
            ),
            active: Arc::new(active),
            len,
        };
        Ok(catalog)
    }

    pub(crate) fn allocated_files(&self) -> usize {
        self.len as usize
    }

    pub(crate) fn construction_complete(&self) -> bool {
        self.construction_complete
    }

    pub(crate) fn repository_root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn set_construction_complete(&mut self, complete: bool) {
        self.construction_complete = complete;
    }

    pub(crate) fn rebase_in_pool(
        &mut self,
        base: Arc<Repository>,
        pool: Option<&ThreadPool>,
    ) -> Result<(), String> {
        if self.len as usize != base.files().files.len() {
            return Err("catalog rebase changed discovered files".into());
        }
        let configuration = Self::configuration_for(&base)?;
        let complete = self.construction_complete;
        let ownership = Arc::clone(&self.ownership);
        let fingerprints = &base.base_fingerprints;
        let files = &base.files;
        let paths_changed = AtomicBool::new(false);
        let metadata = match pool {
            Some(pool) => pool.install(|| {
                (0..self.metadata.len())
                    .into_par_iter()
                    .map(|number| {
                        let first = (number as u32)
                            .checked_mul(FILE_SHARD_WIDTH)
                            .ok_or("catalog shard ID overflow")?;
                        if first >= self.len {
                            return Ok(Some(empty_metadata_shard()));
                        }
                        let end = self
                            .len
                            .min(first.saturating_add(FILE_SHARD_WIDTH));
                        let current_shard = &self.metadata[number];
                        let mut changed = false;
                        for id in first..end {
                            let current = files
                                .files
                                .get(id as usize)
                                .ok_or("catalog rebase file row missing")?;
                            let fingerprint =
                                fingerprints.get(id as usize).copied().flatten();
                            let entry_configuration =
                                fingerprint.map(|_| configuration);
                            let archived = current_shard.archived_row(FileId(id));
                            if archived.is_none_or(|archived| {
                                !crate::archived_path::matches_native(
                                    &archived.path,
                                    current.relative_path.as_path(),
                                )
                            }) {
                                paths_changed.store(true, AtomicOrdering::Relaxed);
                            }
                            let row_matches = archived.is_some_and(|archived| {
                                archived_catalog_row_matches(
                                    archived,
                                    current,
                                    fingerprint,
                                    entry_configuration,
                                )
                            });
                            if !row_matches {
                                changed = true;
                            }
                        }
                        if !changed {
                            return Ok(None);
                        }
                        let rows = (first..end)
                            .map(|id| {
                                let entry = files
                                    .files
                                    .get(id as usize)
                                    .ok_or("catalog rebase file row missing")?;
                                let fingerprint =
                                    fingerprints.get(id as usize).copied().flatten();
                                Ok(StoredFileRow::from_entry(
                                    entry,
                                    fingerprint,
                                    fingerprint.map(|_| configuration),
                                ))
                            })
                            .collect::<Result<Vec<_>, String>>()?;
                        build_metadata_shard(rows).map(Some)
                    })
                    .collect::<Result<Vec<_>, String>>()
            })?,
            None => {
                let mut metadata = Vec::with_capacity(self.metadata.len());
                for number in 0..self.metadata.len() {
                    let first = (number as u32)
                        .checked_mul(FILE_SHARD_WIDTH)
                        .ok_or("catalog shard ID overflow")?;
                    if first >= self.len {
                        metadata.push(Some(empty_metadata_shard()));
                        continue;
                    }
                    let end = self.len.min(first.saturating_add(FILE_SHARD_WIDTH));
                    let current_shard = &self.metadata[number];
                    let mut changed = false;
                    for id in first..end {
                        let current = files
                            .files
                            .get(id as usize)
                            .ok_or("catalog rebase file row missing")?;
                        let fingerprint =
                            fingerprints.get(id as usize).copied().flatten();
                        let entry_configuration = fingerprint.map(|_| configuration);
                        let archived = current_shard.archived_row(FileId(id));
                        if archived.is_none_or(|archived| {
                            !crate::archived_path::matches_native(
                                &archived.path,
                                current.relative_path.as_path(),
                            )
                        }) {
                            paths_changed.store(true, AtomicOrdering::Relaxed);
                        }
                        let row_matches = archived.is_some_and(|archived| {
                            archived_catalog_row_matches(
                                archived,
                                current,
                                fingerprint,
                                entry_configuration,
                            )
                        });
                        if !row_matches {
                            changed = true;
                        }
                    }
                    if !changed {
                        metadata.push(None);
                        continue;
                    }
                    let rows = (first..end)
                        .map(|id| {
                            let entry = files
                                .files
                                .get(id as usize)
                                .ok_or("catalog rebase file row missing")?;
                            let fingerprint =
                                fingerprints.get(id as usize).copied().flatten();
                            Ok(StoredFileRow::from_entry(
                                entry,
                                fingerprint,
                                fingerprint.map(|_| configuration),
                            ))
                        })
                        .collect::<Result<Vec<_>, String>>()?;
                    metadata.push(Some(build_metadata_shard(rows)?));
                }
                metadata
            }
        };
        let metadata = metadata
            .into_iter()
            .enumerate()
            .map(|(number, replacement)| {
                replacement.unwrap_or_else(|| Arc::clone(&self.metadata[number]))
            })
            .collect::<Vec<_>>();
        let paths = if paths_changed.load(AtomicOrdering::Relaxed) {
            let mut buckets = vec![Vec::new(); SHARDS];
            let mut seen = HashSet::with_capacity(files.files.len());
            for entry in &files.files {
                if !valid_relative_path(entry.relative_path.as_path())
                    || !seen.insert(entry.id)
                {
                    return Err("invalid or duplicate rebased catalog path".into());
                }
                buckets[shard(&entry.relative_path.query_key())].push((
                    entry.relative_path.query_key().to_string(),
                    entry.id,
                ));
            }
            let build = |entries: Vec<(String, FileId)>| {
                if entries.is_empty() {
                    empty_path_shard()
                } else {
                    build_path_shard(entries).expect("file inventory paths are unique")
                }
            };
            match pool {
                Some(pool) => pool.install(|| {
                    buckets
                        .into_par_iter()
                        .map(build)
                        .collect::<Vec<_>>()
                }),
                None => buckets.into_iter().map(build).collect(),
            }
        } else {
            (*self.paths).clone()
        };
        self.root = files.root.clone();
        self.inventory = Arc::clone(&base.files);
        self.metadata = Arc::new(metadata);
        self.paths = Arc::new(paths);
        self.construction_complete = complete;
        self.ownership = ownership;
        Ok(())
    }

    pub(crate) fn active_files(&self) -> u64 {
        self.active.len()
    }

    pub(crate) fn is_active(&self, file: FileId) -> bool {
        self.metadata
            .get(file_shard_number(file))
            .is_some_and(|shard| shard.row_ordinal(file).is_some())
    }

    pub(crate) fn entry(&self, file: FileId) -> Option<&FileEntry> {
        let shard = self.metadata.get(file_shard_number(file))?;
        let ordinal = shard.row_ordinal(file)?;
        shard.entries().get(ordinal)
    }

    pub(crate) fn entry_owned(&self, file: FileId) -> Option<FileEntry> {
        let shard = self.metadata.get(file_shard_number(file))?;
        let row = shard.archived_row(file)?;
        crate::storage_usage::StorageWorkCounters::update(|counters| {
            counters.metadata_rows_examined =
                counters.metadata_rows_examined.saturating_add(1);
            counters.native_compatibility_copies =
                counters.native_compatibility_copies.saturating_add(1);
        });
        StoredFileRow::from_archived(row).to_entry().ok()
    }

    pub(crate) fn matches_revision(
        &self,
        file: FileId,
        revision: SourceRevision,
        created_ns: Option<i128>,
        collection: CollectionId,
    ) -> bool {
        let Some(shard) = self.metadata.get(file_shard_number(file)) else {
            return false;
        };
        let Some(row) = shard.archived_row(file) else {
            return false;
        };
        crate::storage_usage::StorageWorkCounters::update(|counters| {
            counters.metadata_rows_examined =
                counters.metadata_rows_examined.saturating_add(1);
        });
        row.modified_ns.to_native() == revision.modified_ns
            && row.size.to_native() == revision.size
            && row.created_ns.as_ref().map(|value| value.to_native()) == created_ns
            && row
                .collections
                .iter()
                .any(|candidate| candidate.to_native() == collection.0)
    }

    pub(crate) fn matches_segment_file(
        &self,
        file: FileId,
        collection: CollectionId,
        path: &rkyv::Archived<StoredPath>,
        revision: SourceRevision,
        created_ns: Option<i128>,
    ) -> bool {
        self.metadata
            .get(file_shard_number(file))
            .and_then(|shard| shard.archived_row(file))
            .is_some_and(|row| {
                crate::archived_path::archived_paths_equal(&row.path, path)
                    && row.modified_ns.to_native() == revision.modified_ns
                    && row.size.to_native() == revision.size
                    && row.created_ns.as_ref().map(|value| value.to_native()) == created_ns
                    && row
                        .collections
                        .iter()
                        .any(|candidate| candidate.to_native() == collection.0)
            })
    }

    pub(crate) fn collections(
        &self,
        file: FileId,
    ) -> Option<impl Iterator<Item = CollectionId> + '_> {
        self.metadata
            .get(file_shard_number(file))?
            .archived_row(file)
            .map(|row| {
                row.collections
                    .iter()
                    .map(|collection| CollectionId(collection.to_native()))
            })
    }

    pub(crate) fn status_code(&self, file: FileId) -> Option<u8> {
        self.metadata
            .get(file_shard_number(file))?
            .archived_row(file)
            .map(|row| row.status)
    }

    pub(crate) fn fingerprint(&self, file: FileId) -> Option<[u8; 32]> {
        let bytes = self
            .metadata
            .get(file_shard_number(file))?
            .archived_row(file)?
            .fingerprint
            .as_ref()?;
        bytes.as_slice().try_into().ok()
    }

    pub(crate) fn configuration(&self, file: FileId) -> Option<[u8; 32]> {
        let bytes = self
            .metadata
            .get(file_shard_number(file))?
            .archived_row(file)?
            .configuration
            .as_ref()?;
        bytes.as_slice().try_into().ok()
    }

    pub(crate) fn id_for(&self, path: &Path) -> Option<FileId> {
        let query = crate::path_repr::query_path(path);
        let number = shard(&query);
        let id = self.paths[number]
            .archive
            .root()
            .exact
            .get::<str>(query.as_str())
            .copied()
            .map(|id| FileId(id.to_native()))?;
        let row = self
            .metadata
            .get(file_shard_number(id))?
            .archived_row(id)?;
        crate::archived_path::matches_native(&row.path, path).then_some(id)
    }

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
        bounds: (Bound<Arc<str>>, Bound<Arc<str>>),
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
        bounds: (Bound<Arc<str>>, Bound<Arc<str>>),
        limit: usize,
    ) -> Option<Vec<FileId>> {
        self.path_directory().ids_in_bounds_up_to(bounds, limit)
    }

    pub(crate) fn path_directory(&self) -> PathDirectory {
        PathDirectory {
            shards: Arc::clone(&self.paths),
        }
    }

    #[cfg(test)]
    pub(crate) fn scan_stamps(&self) -> BTreeMap<TestPathBuf, FileStamp> {
        let mut stamps = BTreeMap::new();
        for id in self.active.iter() {
            let Some(entry) = self.entry(FileId(id)) else {
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
        let entry = self.entry(file).ok_or("unknown active file")?.clone();
        let mut next = self.clone();
        next.replace_metadata_entry(file, None)?;
        next.rebuild_path_buckets(&[entry.relative_path.query_key()], &[(&entry, false)])
            .map_err(|error| error.to_string())?;
        Arc::make_mut(&mut next.active).remove(file.0);
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
        validate_entry(&entry, self.len as usize)?;
        let mut next = self.clone();
        next.ensure_metadata_shard(file_shard_number(entry.id));
        next.replace_metadata_entry(
            entry.id,
            Some(StoredFileRow::from_entry(&entry, fingerprint, configuration)),
        )?;
        let query: Arc<str> = entry.relative_path.query_key();
        next.rebuild_path_buckets(&[query], &[(&entry, true)])?;
        Arc::make_mut(&mut next.active).insert(entry.id.0);
        next.len = next.len.checked_add(1).ok_or("too many indexed files")?;
        Ok(next)
    }

    pub(crate) fn replaced(
        &self,
        entry: FileEntry,
        fingerprint: Option<[u8; 32]>,
        configuration: Option<[u8; 32]>,
    ) -> Result<Self, String> {
        let old = self.entry(entry.id).ok_or("unknown active file")?.clone();
        validate_entry(&entry, entry.id.0 as usize)?;
        if self
            .id_for(&entry.relative_path)
            .is_some_and(|other| other != entry.id)
        {
            return Err("file path collision".into());
        }
        let mut next = self.clone();
        next.replace_metadata_entry(
            entry.id,
            Some(StoredFileRow::from_entry(&entry, fingerprint, configuration)),
        )?;
        let old_query = old.relative_path.query_key();
        let new_query = entry.relative_path.query_key();
        if old_query != new_query {
            next.rebuild_path_buckets(
                &[old_query, new_query],
                &[(&old, false), (&entry, true)],
            )?;
        }
        Ok(next)
    }

    fn ensure_metadata_shard(&mut self, number: usize) {
        let metadata = Arc::make_mut(&mut self.metadata);
        while metadata.len() <= number {
            metadata.push(empty_metadata_shard());
        }
    }

    fn replace_metadata_entry(
        &mut self,
        file: FileId,
        replacement: Option<StoredFileRow>,
    ) -> Result<(), String> {
        self.ensure_metadata_shard(file_shard_number(file));
        let number = file_shard_number(file);
        let current = &self.metadata[number];
        crate::storage_usage::StorageWorkCounters::update(|counters| {
            counters.metadata_rows_examined = counters
                .metadata_rows_examined
                .saturating_add(current.archive.root().rows.len() as u64);
        });
        let mut rows = current
            .archive
            .root()
            .rows
            .iter()
            .filter(|row| row.id.to_native() != file.0)
            .map(StoredFileRow::from_archived)
            .collect::<Vec<_>>();
        if let Some(row) = replacement {
            row.to_entry()?;
            rows.push(row);
        }
        let shard = build_metadata_shard(rows)?;
        Arc::make_mut(&mut self.metadata)[number] = shard;
        Ok(())
    }

    /// Apply metadata-only edits once per touched ID shard. Initial file
    /// publication uses this for merged statuses and diagnostics without
    /// materializing healthy rows or rebuilding path owners.
    pub(crate) fn replace_metadata_entries(
        &mut self,
        replacements: Vec<FileEntry>,
    ) -> Result<(), String> {
        if replacements.is_empty() {
            return Ok(());
        }
        let mut grouped =
            BTreeMap::<usize, Vec<(FileId, StoredFileRow)>>::new();
        for entry in replacements {
            validate_entry(&entry, entry.id.0 as usize)?;
            let number = file_shard_number(entry.id);
            let current = self
                .metadata
                .get(number)
                .and_then(|shard| shard.archived_row(entry.id))
                .ok_or("metadata replacement targets an inactive file")?;
            if !crate::archived_path::matches_native(
                &current.path,
                entry.relative_path.as_path(),
            ) {
                return Err("metadata-only replacement changed a file path".into());
            }
            grouped.entry(number).or_default().push((
                entry.id,
                StoredFileRow::from_entry(
                    &entry,
                    self.fingerprint(entry.id),
                    self.configuration(entry.id),
                ),
            ));
        }
        let metadata = Arc::make_mut(&mut self.metadata);
        for (number, replacements) in grouped {
            let current = metadata
                .get(number)
                .ok_or("metadata replacement shard missing")?;
            crate::storage_usage::StorageWorkCounters::update(|counters| {
                counters.metadata_rows_examined = counters
                    .metadata_rows_examined
                    .saturating_add(current.archive.root().rows.len() as u64);
            });
            let replaced = replacements
                .iter()
                .map(|(file, _)| *file)
                .collect::<HashSet<_>>();
            let mut rows = current
                .archive
                .root()
                .rows
                .iter()
                .filter(|row| !replaced.contains(&FileId(row.id.to_native())))
                .map(StoredFileRow::from_archived)
                .collect::<Vec<_>>();
            rows.extend(replacements.into_iter().map(|(_, row)| row));
            let rebuilt = build_metadata_shard(rows)?;
            metadata[number] = rebuilt;
        }
        Ok(())
    }

    fn rebuild_path_buckets(
        &mut self,
        affected_query_keys: &[Arc<str>],
        changes: &[(&FileEntry, bool)],
    ) -> Result<(), String> {
        let buckets = affected_query_keys
            .iter()
            .map(|key| shard(key))
            .collect::<BTreeSet<_>>();
        for bucket in buckets.iter().copied() {
            let mut entries = self.paths[bucket]
                .archive
                .root()
                .ordered
                .iter()
                .map(|(query, id)| {
                    (
                        FileId(id.to_native()),
                        query.as_str().to_owned(),
                    )
                })
                .collect::<BTreeMap<_, _>>();
            crate::storage_usage::StorageWorkCounters::update(|counters| {
                counters.path_entries_examined = counters
                    .path_entries_examined
                    .saturating_add(entries.len() as u64);
            });
            for (entry, add) in changes {
                let query = entry.relative_path.query_key();
                if shard(&query) != bucket {
                    continue;
                }
                if *add {
                    entries.insert(entry.id, query.to_string());
                } else {
                    if entries
                        .get(&entry.id)
                        .is_some_and(|current| current == query.as_ref())
                    {
                        entries.remove(&entry.id);
                    }
                }
            }
            let rebuilt = build_path_shard(entries.into_iter().map(|(id, query)| (query, id)))?;
            Arc::make_mut(&mut self.paths)[bucket] = rebuilt;
        }
        Ok(())
    }

    pub(crate) fn replace_all_ownership(
        &mut self,
        versions: &FileVersions,
        routes: &[Vec<OwnershipRoute>],
    ) -> Result<(), String> {
        if routes.len() != versions.len() {
            return Err("ownership route table length mismatch".into());
        }
        let shard_count = file_shard_number(
            FileId(versions.len().saturating_sub(1) as u32),
        ) + 1;
        let mut grouped = (0..shard_count)
            .map(|_| Vec::<StoredOwnershipRow>::new())
            .collect::<Vec<_>>();
        for (number, routes) in routes.iter().enumerate() {
            if routes.is_empty() {
                continue;
            }
            let file = FileId(
                u32::try_from(number).map_err(|_| "too many ownership files")?,
            );
            let version = versions
                .get(file)
                .ok_or("ownership file version missing")?;
            grouped[file_shard_number(file)].push(StoredOwnershipRow {
                file: file.0,
                version: version.0,
                routes: routes.iter().copied().map(stored_ownership_route).collect(),
            });
        }
        let ownership = grouped
            .into_iter()
            .map(|rows| {
                if rows.is_empty() {
                    Ok(empty_ownership_shard())
                } else {
                    build_ownership_shard(rows)
                }
            })
            .collect::<Result<Vec<_>, String>>()?;
        self.ownership = Arc::new(ownership);
        Ok(())
    }

    pub(crate) fn ownership_routes(
        &self,
        file: FileId,
        version: FileVersion,
    ) -> Result<Vec<OwnershipRoute>, String> {
        let row = self
            .ownership
            .get(file_shard_number(file))
            .and_then(|shard| shard.row(file))
            .ok_or("current file ownership row missing")?;
        if row.version.to_native() != version.0 {
            return Err("file ownership incarnation mismatch".into());
        }
        let routes = row
            .routes
            .iter()
            .map(archived_ownership_route)
            .collect::<Vec<_>>();
        crate::storage_usage::StorageWorkCounters::update(|counters| {
            counters.ownership_routes_examined =
                counters.ownership_routes_examined.saturating_add(routes.len() as u64);
        });
        Ok(routes)
    }

    pub(crate) fn retire_ownership(
        &mut self,
        file: FileId,
        version: FileVersion,
    ) -> Result<(), String> {
        let number = file_shard_number(file);
        let shard = self
            .ownership
            .get(number)
            .ok_or("file ownership shard missing")?;
        let row = shard.row(file).ok_or("current file ownership row missing")?;
        if row.version.to_native() != version.0 {
            return Err("file ownership incarnation mismatch".into());
        }
        let mut rows = shard
            .archive
            .root()
            .rows
            .iter()
            .filter(|row| row.file.to_native() != file.0)
            .map(|row| StoredOwnershipRow {
                file: row.file.to_native(),
                version: row.version.to_native(),
                routes: row
                    .routes
                    .iter()
                    .map(|route| stored_ownership_route(archived_ownership_route(route)))
                    .collect(),
            })
            .collect::<Vec<_>>();
        let replacement = if rows.is_empty() {
            empty_ownership_shard()
        } else {
            build_ownership_shard(std::mem::take(&mut rows))?
        };
        Arc::make_mut(&mut self.ownership)[number] = replacement;
        Ok(())
    }

    pub(crate) fn advance_ownership(
        &mut self,
        file: FileId,
        expected: FileVersion,
        next: FileVersion,
    ) -> Result<(), String> {
        let number = file_shard_number(file);
        let shard = self
            .ownership
            .get(number)
            .ok_or("file ownership shard missing")?;
        let mut rows = shard
            .archive
            .root()
            .rows
            .iter()
            .map(|row| StoredOwnershipRow {
                file: row.file.to_native(),
                version: row.version.to_native(),
                routes: row
                    .routes
                    .iter()
                    .map(|route| stored_ownership_route(archived_ownership_route(route)))
                    .collect(),
            })
            .collect::<Vec<_>>();
        let row = rows
            .iter_mut()
            .find(|row| row.file == file.0)
            .ok_or("current file ownership row missing")?;
        if row.version != expected.0 {
            return Err("file ownership incarnation mismatch".into());
        }
        row.version = next.0;
        Arc::make_mut(&mut self.ownership)[number] = build_ownership_shard(rows)?;
        Ok(())
    }

    pub(crate) fn update_ownership(
        &mut self,
        versions: &FileVersions,
        removed: &[(FileId, FileVersion, OwnershipRoute)],
        added: &[(FileId, OwnershipRoute)],
    ) -> Result<(), String> {
        let mut touched = BTreeSet::new();
        touched.extend(removed.iter().map(|(file, _, _)| file_shard_number(*file)));
        touched.extend(added.iter().map(|(file, _)| file_shard_number(*file)));
        for number in touched {
            let current = self
                .ownership
                .get(number)
                .ok_or("ownership shard missing")?;
            let mut rows = current
                .archive
                .root()
                .rows
                .iter()
                .map(|row| {
                    (
                        row.file.to_native(),
                        (
                        row.version.to_native(),
                        row.routes
                            .iter()
                            .map(archived_ownership_route)
                            .collect::<Vec<_>>(),
                        ),
                    )
                })
                .collect::<BTreeMap<_, _>>();
            for (file, version, route) in removed {
                if file_shard_number(*file) != number {
                    continue;
                }
                let Some(row) = rows.get_mut(&file.0) else {
                    continue;
                };
                if row.0 != version.0 {
                    return Err(format!(
                        "file ownership incarnation mismatch for {}: expected {}, found {}",
                        file.0, version.0, row.0
                    ));
                }
                row.1.retain(|candidate| candidate != route);
            }
            for (file, route) in added {
                if file_shard_number(*file) != number {
                    continue;
                }
                let version = versions
                    .get(*file)
                    .ok_or("updated ownership file version missing")?;
                let row = rows.entry(file.0).or_insert_with(|| {
                    (version.0, Vec::new())
                });
                if row.0 != version.0 {
                    return Err("new ownership route has stale file version".into());
                }
                if !row.1.contains(route) {
                    row.1.push(*route);
                }
            }
            let replacement = if rows.values().all(|(_, routes)| routes.is_empty()) {
                empty_ownership_shard()
            } else {
                build_ownership_shard(rows.into_iter().filter_map(
                    |(file, (version, routes))| {
                        (!routes.is_empty()).then(|| StoredOwnershipRow {
                            file,
                            version,
                            routes: routes
                                .into_iter()
                                .map(stored_ownership_route)
                                .collect(),
                        })
                    },
                ))?
            };
            Arc::make_mut(&mut self.ownership)[number] = replacement;
        }
        Ok(())
    }

    pub(crate) fn collect_storage_usage(
        &self,
        seen: &mut HashSet<usize>,
        usage: &mut crate::storage_usage::StorageUsage,
    ) {
        for shard in self.metadata.iter() {
            usage.record_archive(
                seen,
                Arc::as_ptr(shard) as usize,
                shard.archive.len(),
                shard.archive.capacity(),
            );
            if let Some(cache) = shard.cache.get() {
                usage.file_row_cache_bytes = usage
                    .file_row_cache_bytes
                    .saturating_add(cache.len() as u64);
                usage.metadata_cache_initializations =
                    usage.metadata_cache_initializations.saturating_add(1);
            }
        }
        for shard in self.paths.iter() {
            usage.record_archive(
                seen,
                Arc::as_ptr(shard) as usize,
                shard.archive.len(),
                shard.archive.capacity(),
            );
        }
        for shard in self.ownership.iter() {
            usage.record_archive(
                seen,
                Arc::as_ptr(shard) as usize,
                shard.archive.len(),
                shard.archive.capacity(),
            );
        }
    }
}

#[cfg(test)]
#[path = "lifecycle_catalog.test.rs"]
mod tests;
