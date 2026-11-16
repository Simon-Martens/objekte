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
use std::sync::{Arc, OnceLock};

use rkyv::{Archive, Serialize as ArchiveSerialize};
use roaring::RoaringBitmap;

use crate::archive_storage::ValidatedArchive;
use crate::inventory::{
    ByteRange, FileEntry, FileStatus, SharedPath, SourceDiagnostic, SourceRevision,
    empty_path_bounds, relative_path_bounds,
};
use crate::lifecycle_scan::FileStamp;
use crate::model::{CollectionId, FileId, RecordSlot};
use crate::query::Operator;
use crate::repository::Repository;
use crate::source::DiagnosticCategory;

const SHARDS: usize = 1024;
const FILE_SHARD_WIDTH: u32 = 1024;

fn shard<T: Hash>(value: &T) -> usize {
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
struct StoredCatalogPath {
    query: String,
    #[cfg(unix)]
    native: Vec<u8>,
    #[cfg(windows)]
    native: Vec<u16>,
}

impl StoredCatalogPath {
    fn from_path(path: &SharedPath) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            Self {
                query: path.query_key().to_string(),
                native: path.as_path().as_os_str().as_bytes().to_vec(),
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            Self {
                query: path.query_key().to_string(),
                native: path.as_path().as_os_str().encode_wide().collect(),
            }
        }
    }

    fn to_path(&self) -> Result<SharedPath, String> {
        #[cfg(unix)]
        {
            use std::ffi::OsString;
            use std::os::unix::ffi::OsStringExt;
            let path = PathBuf::from(OsString::from_vec(self.native.clone()));
            let shared = SharedPath::from(path);
            if shared.query_key().as_ref() != self.query {
                return Err("archived catalog path does not round-trip losslessly".into());
            }
            Ok(shared)
        }
        #[cfg(windows)]
        {
            use std::ffi::OsString;
            use std::os::windows::ffi::OsStringExt;
            let path = PathBuf::from(OsString::from_wide(&self.native));
            let shared = SharedPath::from(path);
            if shared.query_key().as_ref() != self.query {
                return Err("archived catalog path does not round-trip losslessly".into());
            }
            Ok(shared)
        }
    }
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
}

#[derive(Archive, ArchiveSerialize)]
struct StoredFileRow {
    id: u32,
    path: StoredCatalogPath,
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
            path: StoredCatalogPath::from_path(&entry.relative_path),
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

    fn to_entry(&self) -> Result<FileEntry, String> {
        let status = match self.status {
            0 => FileStatus::Valid,
            1 => FileStatus::Broken,
            2 => FileStatus::Unreadable,
            _ => return Err("archived file status is invalid".into()),
        };
        let entry = FileEntry {
            id: FileId(self.id),
            relative_path: self.path.to_path()?,
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

/// One published view's active file table. Previous views retain their own
/// archived shards and liveness bitmap after a new root is published.
#[derive(Clone)]
pub(crate) struct FileCatalog {
    root: PathBuf,
    construction_complete: bool,
    metadata: Arc<Vec<Arc<CatalogMetadataShard>>>,
    paths: Arc<Vec<Arc<CatalogPathShard>>>,
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

fn build_metadata_shard(
    rows: impl IntoIterator<Item = StoredFileRow>,
) -> Result<Arc<CatalogMetadataShard>, String> {
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
        row.to_entry()?;
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

fn empty_metadata_shard() -> Arc<CatalogMetadataShard> {
    build_metadata_shard(std::iter::empty()).expect("empty catalog shard serializes")
}

fn empty_path_shard() -> Arc<CatalogPathShard> {
    Arc::new(CatalogPathShard {
        archive: ValidatedArchive::freeze(&CatalogPathArchive {
            exact: HashMap::new(),
            ordered: BTreeMap::new(),
        })
        .expect("empty path shard serializes"),
    })
}

fn build_path_shard(
    entries: impl IntoIterator<Item = (String, FileId)>,
) -> Result<Arc<CatalogPathShard>, String> {
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
            self.archive
                .root()
                .rows
                .iter()
                .map(|row| {
                    let id = row.id.to_native();
                    let path = archived_path_to_shared(&row.path).expect("validated archived path");
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
        files: Arc<crate::inventory::FileInventory>,
        fingerprints: Vec<Option<[u8; 32]>>,
        configuration_identity: [u8; 32],
    ) -> Self {
        let len = u32::try_from(files.files.len()).expect("file inventory length checked at build");
        let mut metadata = (0..=file_shard_number(FileId(len.saturating_sub(1))))
            .map(|_| empty_metadata_shard())
            .collect::<Vec<_>>();
        let mut path_buckets = vec![Vec::new(); SHARDS];
        let mut grouped_rows = (0..metadata.len()).map(|_| Vec::new()).collect::<Vec<_>>();
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
        for (number, rows) in grouped_rows.into_iter().enumerate() {
            if !rows.is_empty() {
                metadata[number] = build_metadata_shard(rows)
                    .expect("file inventory entries pass catalog validation");
            }
        }
        let mut paths = (0..SHARDS)
            .map(|_| empty_path_shard())
            .collect::<Vec<_>>();
        for (number, entries) in path_buckets.into_iter().enumerate() {
            if !entries.is_empty() {
                paths[number] =
                    build_path_shard(entries).expect("file inventory paths are unique");
            }
        }
        Self {
            root: files.root.clone(),
            construction_complete: true,
            metadata: Arc::new(metadata),
            paths: Arc::new(paths),
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
            metadata: Arc::new(metadata),
            paths: Arc::new(paths),
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

    pub(crate) fn rebase(&mut self, base: Arc<Repository>) -> Result<(), String> {
        if self.len as usize != base.files().files.len() {
            return Err("catalog rebase changed discovered files".into());
        }
        let configuration = Self::configuration_for(&base)?;
        let complete = self.construction_complete;
        *self = Self::from_parts(
            Arc::clone(&base.files),
            base.base_fingerprints.clone(),
            configuration,
        );
        self.construction_complete = complete;
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
        archived_path_matches(&row.path, path).then_some(id)
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
        if empty_path_bounds(&bounds) {
            return Some(Vec::new());
        }
        let bounds = string_bounds(&bounds);
        let mut rows = Vec::new();
        let mut scanned = 0usize;
        for path_shard in self.paths.iter() {
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
                rows.push((query.as_str().to_owned(), FileId(id.to_native())));
            }
        }
        rows.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
        Some(rows.into_iter().map(|(_, id)| id).collect())
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
        next.rebuild_path_buckets(
            &[old.relative_path.query_key(), entry.relative_path.query_key()],
            &[(&old, false), (&entry, true)],
        )?;
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
        let mut rows = current
            .entries()
            .iter()
            .filter(|entry| entry.id != file)
            .map(|entry| {
                let row = current
                    .archived_row(entry.id)
                    .expect("cached metadata entry has archived row");
                let fingerprint = row
                    .fingerprint
                    .as_ref()
                    .and_then(|bytes| bytes.as_slice().try_into().ok());
                let configuration = row
                    .configuration
                    .as_ref()
                    .and_then(|bytes| bytes.as_slice().try_into().ok());
                (entry.clone(), fingerprint, configuration)
            })
            .collect::<Vec<_>>();
        if let Some(row) = replacement {
            let old_fingerprint = row.fingerprint.as_deref().and_then(|bytes| bytes.try_into().ok());
            let old_configuration =
                row.configuration.as_deref().and_then(|bytes| bytes.try_into().ok());
            let entry = row.to_entry()?;
            rows.push((entry, old_fingerprint, old_configuration));
        }
        let replacement_rows = rows.iter().map(|(entry, fingerprint, configuration)| {
            StoredFileRow::from_entry(entry, *fingerprint, *configuration)
        });
        let shard = build_metadata_shard(replacement_rows)?;
        Arc::make_mut(&mut self.metadata)[number] = shard;
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
            let mut entries = BTreeMap::new();
            for metadata_shard in self.metadata.iter() {
                for entry in metadata_shard.entries() {
                    if shard(&entry.relative_path.query_key()) == bucket {
                        entries.insert(entry.id, entry.relative_path.query_key().to_string());
                    }
                }
            }
            for (entry, add) in changes {
                let query = entry.relative_path.query_key();
                if shard(&query) != bucket {
                    continue;
                }
                if *add {
                    entries.insert(entry.id, query.to_string());
                } else {
                    entries.remove(&entry.id);
                }
            }
            let rebuilt = build_path_shard(entries.into_iter().map(|(id, query)| (query, id)))?;
            Arc::make_mut(&mut self.paths)[bucket] = rebuilt;
        }
        Ok(())
    }

    pub(crate) fn archive_bytes(&self) -> usize {
        let mut seen = HashSet::<usize>::new();
        self.metadata
            .iter()
            .map(|shard| Arc::as_ptr(shard) as usize)
            .chain(self.paths.iter().map(|shard| Arc::as_ptr(shard) as usize))
            .filter(|owner| seen.insert(*owner))
            .map(|owner| {
                let metadata = self
                    .metadata
                    .iter()
                    .find(|shard| Arc::as_ptr(shard) as usize == owner);
                metadata.map_or_else(
                    || {
                        self.paths
                            .iter()
                            .find(|shard| Arc::as_ptr(shard) as usize == owner)
                            .map_or(0, |shard| shard.archive.len())
                    },
                    |shard| shard.archive.len(),
                )
            })
            .sum()
    }
}

fn archived_path_matches(path: &rkyv::Archived<StoredCatalogPath>, native: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        path.native.as_slice() == native.as_os_str().as_bytes()
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        path.native.iter().copied().eq(native.as_os_str().encode_wide())
    }
}

fn archived_path_to_shared(
    path: &rkyv::Archived<StoredCatalogPath>,
) -> Result<SharedPath, String> {
    #[cfg(unix)]
    {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;
        let shared = SharedPath::from(PathBuf::from(OsString::from_vec(
            path.native.as_slice().to_vec(),
        )));
        if shared.query_key().as_ref() != path.query.as_str() {
            return Err("archived catalog path does not round-trip losslessly".into());
        }
        Ok(shared)
    }
    #[cfg(windows)]
    {
        use std::ffi::OsString;
        use std::os::windows::ffi::OsStringExt;
        let native = path.native.iter().map(|unit| unit.to_native()).collect::<Vec<_>>();
        let shared = SharedPath::from(PathBuf::from(OsString::from_wide(&native)));
        if shared.query_key().as_ref() != path.query.as_str() {
            return Err("archived catalog path does not round-trip losslessly".into());
        }
        Ok(shared)
    }
}

#[cfg(test)]
#[path = "lifecycle_catalog.test.rs"]
mod tests;
