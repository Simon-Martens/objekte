//! Native file and record directories, version tokens, and liveness.

use std::collections::{BTreeMap, HashMap};
use std::ops::Bound;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use roaring::RoaringBitmap;
use serde::{Deserialize, Serialize};

use crate::inventory::{
    ByteRange, DiagnosticRef, FileEntry, FileStatus, FileStorageDiagnostic, FileVersion,
    MAX_DIAGNOSTIC_CODE_BYTES, MAX_DIAGNOSTIC_MESSAGE_BYTES, MAX_DIAGNOSTIC_PATH_SEGMENTS,
    MAX_DIAGNOSTIC_SEGMENT_BYTES, MAX_DIAGNOSTICS_PER_FILE, RecordClass, RecordEntry, SharedPath,
    SourceDiagnostic, SourceRevision, empty_path_bounds, relative_path_bounds,
};
use crate::model::{CollectionId, FileId, IdentitySet, RecordId, RecordSlot};
use crate::query::Operator;
use crate::source::DiagnosticCategory;

use super::file_scalar::FileScalarIndex;

/// Append a batch-local bitmap after its final slot base is known. Batch
/// admission is ordered, so each translated set must follow existing slots.
pub(crate) fn append_shifted_bitmap(
    destination: &mut RoaringBitmap,
    source: &RoaringBitmap,
    base: u32,
) -> Result<(), String> {
    if let Some(max) = source.max() {
        base.checked_add(max).ok_or("record slot overflow")?;
    }
    destination
        .append(source.iter().map(|slot| base + slot))
        .map_err(|_| "batch bitmap slots overlap or are out of order")?;
    Ok(())
}

const FILE_VERSION_CHUNK: usize = 1024;

/// Version metadata is divided into shared chunks so changing one file does
/// not copy every file's token. The outer pointer directory is shallow.
#[derive(Clone, Debug)]
pub(crate) struct FileVersions {
    chunks: Arc<Vec<Arc<Vec<FileVersion>>>>,
    len: usize,
}

impl FileVersions {
    /// Restore version tokens without replaying every historical mutation.
    #[cfg(test)]
    pub(crate) fn from_versions(versions: Vec<FileVersion>) -> Result<Self, String> {
        let len = versions.len();
        u32::try_from(len).map_err(|_| "too many indexed file versions")?;
        if versions.iter().any(|version| version.0 == 0) {
            return Err("file version must be nonzero".into());
        }
        let chunks = versions
            .chunks(FILE_VERSION_CHUNK)
            .map(|chunk| Arc::new(chunk.to_vec()))
            .collect();
        Ok(Self {
            chunks: Arc::new(chunks),
            len,
        })
    }

    #[cfg(test)]
    pub(crate) fn initial(len: usize) -> Self {
        Self::filled(len, FileVersion::INITIAL)
    }

    pub(crate) fn filled(len: usize, version: FileVersion) -> Self {
        let chunks = (0..len)
            .step_by(FILE_VERSION_CHUNK)
            .map(|start| Arc::new(vec![version; (len - start).min(FILE_VERSION_CHUNK)]))
            .collect();
        Self {
            chunks: Arc::new(chunks),
            len,
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn get(&self, file: FileId) -> Option<FileVersion> {
        let index = file.0 as usize;
        if index >= self.len {
            return None;
        }
        self.chunks
            .get(index / FILE_VERSION_CHUNK)?
            .get(index % FILE_VERSION_CHUNK)
            .copied()
    }

    pub(super) fn read_batch(
        &self,
        start: FileId,
        limit: crate::index::storage::transfer::BatchLimit,
    ) -> crate::index::storage::transfer::StorageResult<(
        std::borrow::Cow<'_, [FileVersion]>,
        crate::index::storage::transfer::BatchStatus,
    )> {
        use crate::index::storage::transfer::{BatchStatus, StorageError};
        const OP: &str = "read native file versions";
        let start = start.0 as usize;
        if start == self.len {
            return Ok((std::borrow::Cow::Borrowed(&[]), BatchStatus::Done));
        }
        let chunk = self
            .chunks
            .get(start / FILE_VERSION_CHUNK)
            .and_then(|chunk| chunk.get(start % FILE_VERSION_CHUNK..))
            .ok_or_else(|| StorageError::invalid(OP, "version start out of bounds"))?;
        let length = chunk
            .len()
            .min(limit.fixed_batch_len(OP, size_of::<FileVersion>())?);
        let status = if start + length == self.len {
            BatchStatus::Done
        } else {
            BatchStatus::More
        };
        Ok((std::borrow::Cow::Borrowed(&chunk[..length]), status))
    }

    pub(super) fn from_catalog(
        source: &dyn crate::index::storage::file_catalog::FileCatalogRevision,
        cancelled: &dyn Fn() -> bool,
    ) -> crate::index::storage::transfer::StorageResult<Self> {
        use crate::index::storage::file_catalog::{FILE_TRANSFER_LIMIT, transfer_versions};
        let mut chunks = Vec::new();
        let mut current = Vec::new();
        transfer_versions(source, FILE_TRANSFER_LIMIT, cancelled, |mut versions| {
            while !versions.is_empty() {
                let length = versions.len().min(FILE_VERSION_CHUNK - current.len());
                let (next, rest) = versions.split_at(length);
                current.extend_from_slice(next);
                if current.len() == FILE_VERSION_CHUNK {
                    chunks.push(Arc::new(std::mem::take(&mut current)));
                }
                versions = rest;
            }
            Ok(())
        })?;
        if !current.is_empty() {
            chunks.push(Arc::new(current));
        }
        Ok(Self {
            chunks: Arc::new(chunks),
            len: source.high_water_mark() as usize,
        })
    }

    /// Create a private next version without changing any published view.
    pub(crate) fn advanced(&self, file: FileId) -> Result<(Self, FileVersion), String> {
        let index = file.0 as usize;
        if index >= self.len {
            return Err("file ID out of bounds".into());
        }
        let mut chunks = (*self.chunks).clone();
        let token = Arc::make_mut(&mut chunks[index / FILE_VERSION_CHUNK])
            .get_mut(index % FILE_VERSION_CHUNK)
            .ok_or("file version missing")?;
        *token = token.next()?;
        let next_version = *token;
        Ok((
            Self {
                chunks: Arc::new(chunks),
                len: self.len,
            },
            next_version,
        ))
    }

    /// Assign a never-before-used file ID its first incarnation token.
    pub(crate) fn appended(&self) -> Result<Self, String> {
        u32::try_from(self.len).map_err(|_| "too many files")?;
        let mut chunks = (*self.chunks).clone();
        if self.len % FILE_VERSION_CHUNK == 0 {
            chunks.push(Arc::new(vec![FileVersion::INITIAL]));
        } else {
            Arc::make_mut(chunks.last_mut().ok_or("file version chunk missing")?)
                .push(FileVersion::INITIAL);
        }
        Ok(Self {
            chunks: Arc::new(chunks),
            len: self.len + 1,
        })
    }
}

const ACTIVE_SLOT_CHUNK: u32 = 1 << 16;

/// Published liveness for one collection. Slot numbers are never reused by
/// ordinary mutations. Only touched chunks are copied on retirement.
#[derive(Clone, Debug)]
pub(crate) struct MemoryLiveSlots {
    chunks: Arc<Vec<Arc<RoaringBitmap>>>,
    slot_count: u32,
    live_count: u64,
}

impl MemoryLiveSlots {
    /// Restore a sparse active domain, including fully retired chunks.
    #[cfg(test)]
    pub(crate) fn from_bitmap(slot_count: usize, bitmap: RoaringBitmap) -> Result<Self, String> {
        Self::from_membership(
            slot_count,
            crate::query_access::membership::MembershipView::Borrowed(&bitmap),
        )
    }

    pub(super) fn from_membership(
        slot_count: usize,
        slots: crate::query_access::membership::MembershipView<'_>,
    ) -> Result<Self, String> {
        let slot_count = u32::try_from(slot_count).map_err(|_| "too many records")?;
        if slots.last().is_some_and(|slot| slot >= slot_count) {
            return Err("active record slot out of bounds".into());
        }
        let chunk_count = (slot_count as usize).div_ceil(ACTIVE_SLOT_CHUNK as usize);
        let mut chunks = vec![RoaringBitmap::new(); chunk_count];
        for slot in slots.iter() {
            chunks[(slot / ACTIVE_SLOT_CHUNK) as usize].insert(slot % ACTIVE_SLOT_CHUNK);
        }
        Ok(Self {
            chunks: Arc::new(chunks.into_iter().map(Arc::new).collect()),
            slot_count,
            live_count: slots.len() as u64,
        })
    }

    pub(crate) fn all(slot_count: usize) -> Result<Self, String> {
        let slot_count = u32::try_from(slot_count).map_err(|_| "too many records")?;
        let mut chunks = Vec::new();
        let mut start = 0u32;
        while start < slot_count {
            let end = slot_count.min(start.saturating_add(ACTIVE_SLOT_CHUNK));
            let mut active = RoaringBitmap::new();
            active.insert_range(0..end - start);
            chunks.push(Arc::new(active));
            start = end;
        }
        Ok(Self {
            chunks: Arc::new(chunks),
            slot_count,
            live_count: slot_count as u64,
        })
    }

    pub(crate) fn slot_count(&self) -> u32 {
        self.slot_count
    }

    pub(crate) fn len(&self) -> u64 {
        self.live_count
    }

    pub(crate) fn contains(&self, slot: RecordSlot) -> bool {
        if slot.0 >= self.slot_count {
            return false;
        }
        self.chunks
            .get((slot.0 / ACTIVE_SLOT_CHUNK) as usize)
            .is_some_and(|chunk| chunk.contains(slot.0 % ACTIVE_SLOT_CHUNK))
    }

    pub(crate) fn retired(&self, slots: &[RecordSlot]) -> Result<Self, String> {
        let mut chunks = (*self.chunks).clone();
        let mut live_count = self.live_count;
        for slot in slots {
            if slot.0 >= self.slot_count {
                return Err("record slot out of bounds".into());
            }
            let chunk = Arc::make_mut(&mut chunks[(slot.0 / ACTIVE_SLOT_CHUNK) as usize]);
            if chunk.remove(slot.0 % ACTIVE_SLOT_CHUNK) {
                live_count -= 1;
            }
        }
        Ok(Self {
            chunks: Arc::new(chunks),
            slot_count: self.slot_count,
            live_count,
        })
    }

    #[cfg(test)]
    pub(crate) fn appended(&self, count: u32) -> Result<Self, String> {
        let slot_count = self
            .slot_count
            .checked_add(count)
            .ok_or("record slot overflow")?;
        let mut chunks = (*self.chunks).clone();
        let mut current = self.slot_count;
        while current < slot_count {
            let group = (current / ACTIVE_SLOT_CHUNK) as usize;
            let local = current % ACTIVE_SLOT_CHUNK;
            let take = (slot_count - current).min(ACTIVE_SLOT_CHUNK - local);
            if group == chunks.len() {
                chunks.push(Arc::new(RoaringBitmap::new()));
            }
            Arc::make_mut(&mut chunks[group]).insert_range(local..local + take);
            current += take;
        }
        Ok(Self {
            chunks: Arc::new(chunks),
            slot_count,
            live_count: self.live_count + count as u64,
        })
    }

    #[cfg(test)]
    pub(crate) fn bitmap(&self) -> RoaringBitmap {
        if self.live_count == self.slot_count as u64 {
            let mut full = RoaringBitmap::new();
            full.insert_range(0..self.slot_count);
            return full;
        }
        self.chunks
            .iter()
            .enumerate()
            .flat_map(|(group, chunk)| {
                let base = group as u32 * ACTIVE_SLOT_CHUNK;
                chunk.iter().map(move |slot| base + slot)
            })
            .collect()
    }
}

impl crate::index::storage::liveness::LiveSlotReader for MemoryLiveSlots {
    fn slot_count(&self) -> u32 {
        self.slot_count()
    }

    fn live_count(&self) -> u64 {
        self.len()
    }

    fn contains(&self, slot: RecordSlot) -> bool {
        self.contains(slot)
    }

    fn visit_blocks(
        &self,
        visit: &mut dyn FnMut(u32, crate::query_access::membership::MembershipView<'_>),
    ) {
        for (group, chunk) in self.chunks.iter().enumerate() {
            visit(
                group as u32 * ACTIVE_SLOT_CHUNK,
                crate::query_access::membership::MembershipView::Borrowed(chunk),
            );
        }
    }

    fn retired(
        &self,
        slots: &[RecordSlot],
    ) -> crate::index::storage::transfer::StorageResult<
        crate::index::storage::liveness::ActiveRecords,
    > {
        self.retired(slots)
            .map(crate::index::storage::liveness::ActiveRecords::new)
            .map_err(|error| {
                crate::index::storage::transfer::StorageError::invalid("retire native slots", error)
            })
    }
}

impl SharedPath {
    fn payload_bytes(&self) -> usize {
        self.as_path().as_os_str().len()
    }

    fn query_key_has_separate_payload(&self) -> bool {
        matches!(self, Self::Native(_))
    }
}

/// Authoritative source-file table shared by collections.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileInventory {
    pub root: Option<PathBuf>,
    pub files: Vec<FileEntry>,
    #[serde(skip)]
    by_path: HashMap<SharedPath, FileId>,
    /// Ordered root-relative query keys, rebuilt with every restored index.
    #[serde(skip)]
    pub(crate) relative_paths: BTreeMap<Arc<str>, FileId>,
    #[serde(skip)]
    scalar: Arc<std::sync::OnceLock<FileScalarIndex>>,
}

impl FileInventory {
    pub fn storage_diagnostic(&self) -> FileStorageDiagnostic {
        let native_path_payload_bytes = self
            .files
            .iter()
            .map(|file| file.relative_path.payload_bytes() + 2 * std::mem::size_of::<usize>())
            .sum::<usize>();
        let escaped_query_keys = self
            .files
            .iter()
            .filter(|file| file.relative_path.query_key_has_separate_payload())
            .count();
        let escaped_query_payload_bytes = self
            .files
            .iter()
            .filter(|file| file.relative_path.query_key_has_separate_payload())
            .map(|file| {
                crate::model::path_representation::query_path(&file.relative_path).len()
                    + 2 * std::mem::size_of::<usize>()
            })
            .sum::<usize>();
        let file_table_slot_bytes = self.files.capacity() * std::mem::size_of::<FileEntry>();
        let exact_lookup_slot_bytes =
            self.by_path.capacity() * std::mem::size_of::<(SharedPath, FileId)>();
        let ordered_lookup_slot_bytes =
            self.relative_paths.len() * std::mem::size_of::<(Arc<str>, FileId)>();
        let collection_membership_bytes = self
            .files
            .iter()
            .map(|file| file.collections.capacity() * std::mem::size_of::<CollectionId>())
            .sum::<usize>();
        let root_payload_bytes = self.root.as_ref().map_or(0, PathBuf::capacity);
        let scalar_index_bytes = self
            .scalar
            .get()
            .map_or(0, FileScalarIndex::estimated_heap_bytes);
        FileStorageDiagnostic {
            files_len: self.files.len(),
            files_capacity: self.files.capacity(),
            file_entry_size: std::mem::size_of::<FileEntry>(),
            file_table_slot_bytes,
            native_path_payload_bytes,
            native_path_payload_allocations: self.files.len(),
            exact_lookup_capacity: self.by_path.capacity(),
            exact_lookup_slot_bytes,
            ordered_lookup_len: self.relative_paths.len(),
            ordered_lookup_slot_bytes,
            escaped_query_keys,
            escaped_query_payload_bytes,
            collection_membership_bytes,
            root_payload_bytes,
            scalar_index_bytes,
            estimated_live_bytes: file_table_slot_bytes
                + native_path_payload_bytes
                + exact_lookup_slot_bytes
                + ordered_lookup_slot_bytes
                + escaped_query_payload_bytes
                + collection_membership_bytes
                + root_payload_bytes
                + scalar_index_bytes,
        }
    }

    /// Approximate heap used by inventory and retained diagnostics, separately.
    /// Hash-table control bytes and allocator overhead are not included.
    pub fn estimated_heap_bytes(&self) -> (usize, usize) {
        let mut inventory = self.root.as_ref().map_or(0, PathBuf::capacity)
            + self.files.capacity() * std::mem::size_of::<FileEntry>()
            + self.by_path.capacity() * std::mem::size_of::<(SharedPath, FileId)>();
        let mut diagnostics = 0;
        for file in &self.files {
            inventory += file.relative_path.payload_bytes()
                + file.collections.capacity() * std::mem::size_of::<CollectionId>();
            diagnostics += file.diagnostics.capacity() * std::mem::size_of::<SourceDiagnostic>();
            for diagnostic in &file.diagnostics {
                diagnostics += diagnostic.source_id.capacity()
                    + diagnostic.code.capacity()
                    + diagnostic.message.capacity();
                if let Some(path) = &diagnostic.field_path {
                    diagnostics += path.capacity() * std::mem::size_of::<String>();
                    diagnostics += path.iter().map(String::capacity).sum::<usize>();
                }
            }
        }
        inventory += self.relative_paths.len() * std::mem::size_of::<(Arc<str>, FileId)>();
        inventory += self
            .files
            .iter()
            .filter(|file| file.relative_path.query_key_has_separate_payload())
            .map(|file| crate::model::path_representation::query_path(&file.relative_path).len())
            .sum::<usize>();
        inventory += self
            .scalar
            .get()
            .map_or(0, FileScalarIndex::estimated_heap_bytes);
        (inventory, diagnostics)
    }

    pub fn new(root: PathBuf) -> Self {
        Self {
            root: Some(root),
            files: Vec::new(),
            by_path: HashMap::new(),
            relative_paths: BTreeMap::new(),
            scalar: Arc::new(std::sync::OnceLock::new()),
        }
    }

    /// An index with no file catalog has no filesystem root or file entries.
    pub fn without_root() -> Self {
        Self {
            root: None,
            files: Vec::new(),
            by_path: HashMap::new(),
            relative_paths: BTreeMap::new(),
            scalar: Arc::new(std::sync::OnceLock::new()),
        }
    }

    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    pub(crate) fn required_root(&self) -> &Path {
        self.root()
            .expect("file adapter requires a registered catalog root")
    }

    /// Restore an already decoded file table without first populating lookup
    /// maps through `add`. `rebuild_lookup` validates identities and paths,
    /// constructs both final maps once, and checks diagnostic ownership.
    #[cfg(test)]
    pub(crate) fn from_restored_entries(
        root: PathBuf,
        entries: Vec<FileEntry>,
    ) -> Result<Self, String> {
        let mut inventory = Self {
            root: Some(root),
            files: entries,
            by_path: HashMap::new(),
            relative_paths: BTreeMap::new(),
            scalar: Arc::new(std::sync::OnceLock::new()),
        };
        inventory.rebuild_lookup()?;
        Ok(inventory)
    }

    pub fn add(
        &mut self,
        relative_path: PathBuf,
        revision: SourceRevision,
        created_ns: Option<i128>,
        collection: CollectionId,
    ) -> Result<FileId, String> {
        if self.root.is_none() {
            return Err("file catalog is not registered".into());
        }
        if !valid_relative_path(&relative_path) {
            return Err("file path must be repository-relative and contained".into());
        }
        if let Some(&id) = self.by_path.get(relative_path.as_path()) {
            let file = &mut self.files[id.0 as usize];
            if file.revision != revision {
                return Err("one file has inconsistent revisions".into());
            }
            if !file.collections.contains(&collection) {
                file.collections.push(collection);
            }
            return Ok(id);
        }
        let id = FileId(u32::try_from(self.files.len()).map_err(|_| "too many files")?);
        let relative_path = SharedPath::from(relative_path);
        self.by_path.insert(relative_path.clone(), id);
        self.relative_paths.insert(relative_path.query_key(), id);
        self.files.push(FileEntry {
            id,
            relative_path,
            revision,
            created_ns,
            collections: vec![collection],
            status: FileStatus::Valid,
            diagnostics: Vec::new(),
            omitted_diagnostics: 0,
        });
        self.scalar = Arc::new(std::sync::OnceLock::new());
        Ok(id)
    }

    pub fn record_diagnostic(
        &mut self,
        file: FileId,
        diagnostic: SourceDiagnostic,
    ) -> Result<DiagnosticRef, String> {
        let entry = self
            .files
            .get_mut(file.0 as usize)
            .ok_or("diagnostic file out of bounds")?;
        entry.record_diagnostic(diagnostic)
    }

    pub fn diagnostic(&self, file: FileId, reference: DiagnosticRef) -> Option<&SourceDiagnostic> {
        self.files
            .get(file.0 as usize)?
            .diagnostics
            .get(reference.stored_index()?)
    }

    pub fn validate_diagnostics(&self) -> Result<(), String> {
        for (index, file) in self.files.iter().enumerate() {
            if file.id.0 as usize != index || file.diagnostics.len() > MAX_DIAGNOSTICS_PER_FILE {
                return Err("invalid file diagnostic table".into());
            }
            let derived = file
                .diagnostics
                .iter()
                .fold(FileStatus::Valid, |status, diagnostic| {
                    status.max(match diagnostic.category {
                        DiagnosticCategory::Io => FileStatus::Unreadable,
                        _ => FileStatus::Broken,
                    })
                });
            if (file.omitted_diagnostics == 0 && file.status != derived)
                || (file.omitted_diagnostics != 0
                    && (file.status == FileStatus::Valid
                        || derived == FileStatus::Valid
                        || file.status < derived))
            {
                return Err("file status and diagnostics disagree".into());
            }
            for diagnostic in &file.diagnostics {
                if diagnostic.file != file.id
                    || !file.collections.contains(&diagnostic.collection)
                    || diagnostic.source_id.is_empty()
                    || diagnostic.source_version == 0
                    || diagnostic.code.is_empty()
                    || diagnostic.code.len() > MAX_DIAGNOSTIC_CODE_BYTES
                    || diagnostic.field_path.as_ref().is_some_and(|path| {
                        path.len() > MAX_DIAGNOSTIC_PATH_SEGMENTS
                            || path
                                .iter()
                                .any(|part| part.len() > MAX_DIAGNOSTIC_SEGMENT_BYTES)
                    })
                    || diagnostic.message.len() > MAX_DIAGNOSTIC_MESSAGE_BYTES
                    || diagnostic.range.is_some_and(|range| {
                        range.start > range.end || range.end > file.revision.size
                    })
                    || diagnostic
                        .byte_offset
                        .is_some_and(|offset| offset > file.revision.size)
                {
                    return Err("invalid source diagnostic".into());
                }
            }
        }
        Ok(())
    }

    pub fn rebuild_lookup(&mut self) -> Result<(), String> {
        if self.root.is_none() && !self.files.is_empty() {
            return Err("file entries require a registered catalog root".into());
        }
        self.by_path.clear();
        self.by_path.reserve(self.files.len());
        self.relative_paths.clear();
        self.scalar = Arc::new(std::sync::OnceLock::new());
        for (slot, file) in self.files.iter().enumerate() {
            if !valid_relative_path(&file.relative_path)
                || file.id.0 as usize != slot
                || self
                    .by_path
                    .insert(file.relative_path.clone(), file.id)
                    .is_some()
            {
                return Err("invalid restored file inventory".into());
            }
            if self
                .relative_paths
                .insert(file.relative_path.query_key(), file.id)
                .is_some()
            {
                return Err("invalid restored relative file path index".into());
            }
        }
        self.validate_diagnostics()?;
        Ok(())
    }

    pub fn native_path(&self, id: FileId) -> Option<PathBuf> {
        let root = self.root()?;
        self.files
            .get(id.0 as usize)
            .map(|f| root.join(&f.relative_path))
    }

    pub(crate) fn scalar_index(&self) -> &FileScalarIndex {
        self.scalar
            .get_or_init(|| FileScalarIndex::from_files(&self.files))
    }

    pub fn id_for(&self, path: &Path) -> Option<FileId> {
        self.by_path.get(path).copied()
    }

    pub(crate) fn relative_path_ids_in_bounds(
        &self,
        bounds: (Bound<Arc<str>>, Bound<Arc<str>>),
    ) -> Vec<FileId> {
        if empty_path_bounds(&bounds) {
            return Vec::new();
        }
        self.relative_paths
            .range(bounds)
            .map(|(_, id)| *id)
            .collect()
    }

    pub(crate) fn relative_path_ids_up_to(
        &self,
        operator: Operator,
        literal: &str,
        limit: usize,
    ) -> Option<Vec<FileId>> {
        let bounds = relative_path_bounds(operator, literal)?;
        let mut ids = Vec::new();
        for (_, id) in self.relative_paths.range(bounds) {
            if ids.len() == limit {
                return None;
            }
            ids.push(*id);
        }
        Some(ids)
    }
}

fn valid_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
}

/// Compact per-file offset table. `slots[offsets[f]..offsets[f+1]]` belongs
/// exactly to file `f`; empty files have equal consecutive offsets.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileRecordIndex {
    /// Valid file-ID domain, including empty generations with no offsets.
    pub file_count: u32,
    pub offsets: Vec<u32>,
    pub slots: Vec<RecordSlot>,
}

impl FileRecordIndex {
    pub fn records_for(&self, file: FileId) -> Option<&[RecordSlot]> {
        if file.0 >= self.file_count {
            return None;
        }
        if self.offsets.is_empty() {
            return self.slots.is_empty().then_some(&[]);
        }
        let a = *self.offsets.get(file.0 as usize)? as usize;
        let b = *self.offsets.get(file.0 as usize + 1)? as usize;
        self.slots.get(a..b)
    }
}

/// Immutable record inventory for one published collection.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CollectionGeneration {
    pub collection: CollectionId,
    pub name: String,
    pub generation: u64,
    pub files: Vec<FileId>,
    pub records: Vec<RecordEntry>,
    pub file_records: FileRecordIndex,
}

impl CollectionGeneration {
    pub fn validate(&self, inventory: &FileInventory) -> Result<(), String> {
        let mut included = std::collections::HashSet::new();
        for file_id in &self.files {
            if !included.insert(*file_id)
                || inventory
                    .files
                    .get(file_id.0 as usize)
                    .is_none_or(|file| !file.collections.contains(&self.collection))
            {
                return Err("invalid collection file membership".into());
            }
        }
        if self.file_records.file_count as usize != inventory.files.len()
            || (self.file_records.offsets.is_empty()
                && (!self.records.is_empty() || !self.file_records.slots.is_empty()))
            || (!self.file_records.offsets.is_empty()
                && (self.file_records.offsets.len() != inventory.files.len() + 1
                    || self.file_records.offsets.first() != Some(&0)
                    || self.file_records.offsets.last().copied()
                        != Some(self.file_records.slots.len() as u32)))
        {
            return Err("invalid file-record offset table".into());
        }
        let mut seen = vec![false; self.records.len()];
        for (file_idx, window) in self.file_records.offsets.windows(2).enumerate() {
            let (a, b) = (window[0] as usize, window[1] as usize);
            if a > b || b > self.file_records.slots.len() {
                return Err("file-record offsets out of bounds".into());
            }
            for slot in &self.file_records.slots[a..b] {
                let i = slot.0 as usize;
                let entry = self
                    .records
                    .get(i)
                    .ok_or("file-record slot out of bounds")?;
                if seen[i]
                    || entry.slot != *slot
                    || entry.file.0 as usize != file_idx
                    || !included.contains(&entry.file)
                {
                    return Err("file-record ownership mismatch".into());
                }
                let file = &inventory.files[file_idx];
                if !file.collections.contains(&self.collection)
                    || entry.revision != file.revision
                    || (entry.range.start >= entry.range.end
                        && !(entry.class == RecordClass::RawFallback
                            && entry.range.start == 0
                            && file.revision.size == 0
                            && entry.range.end == 0))
                    || entry.range.end > file.revision.size
                    || (entry.class == RecordClass::Logical
                        && entry.diagnostic_ref != DiagnosticRef(0))
                    || (entry.class == RecordClass::Logical && entry.id.0.starts_with("~fallback:"))
                    || (entry.class == RecordClass::RawFallback
                        && (file.status == FileStatus::Valid
                            || !entry.id.0.starts_with("~fallback:")
                            || entry.diagnostic_ref == DiagnosticRef(0)
                            || entry.diagnostic_ref.stored_index().is_some_and(|i| {
                                file.diagnostics.get(i).is_none_or(|d| {
                                    d.collection != self.collection || d.record != Some(*slot)
                                })
                            })
                            || (entry.diagnostic_ref.omitted() && file.omitted_diagnostics == 0)))
                {
                    return Err("record range or revision mismatch".into());
                }
                seen[i] = true;
            }
        }
        if seen.iter().any(|v| !v) {
            return Err("unmapped record slot".into());
        }
        Ok(())
    }
}

/// Mutable accumulator used to construct a collection generation.
pub struct CollectionBuilder {
    collection: CollectionId,
    name: String,
    generation: u64,
    files: Vec<FileId>,
    included_files: Vec<bool>,
    records: Vec<RecordEntry>,
    ids: IdentitySet,
}

impl CollectionBuilder {
    #[allow(dead_code, reason = "used by retained collection-wide admission")]
    pub(crate) fn next_slot(&self) -> Result<RecordSlot, String> {
        Ok(RecordSlot(
            u32::try_from(self.records.len()).map_err(|_| "too many records")?,
        ))
    }

    pub fn collection_id(&self) -> CollectionId {
        self.collection
    }

    pub fn record(&self, slot: RecordSlot) -> Option<&RecordEntry> {
        self.records.get(slot.0 as usize)
    }

    pub fn new(collection: CollectionId, name: String, generation: u64, file_count: usize) -> Self {
        Self {
            collection,
            name,
            generation,
            files: Vec::new(),
            included_files: vec![false; file_count],
            records: Vec::new(),
            ids: IdentitySet::default(),
        }
    }

    pub fn include_file(&mut self, file: FileId) -> Result<(), String> {
        let included = self
            .included_files
            .get_mut(file.0 as usize)
            .ok_or("file ID out of bounds")?;
        if !*included {
            *included = true;
            self.files.push(file);
        }
        Ok(())
    }

    #[allow(dead_code, reason = "used by retained collection-wide admission")]
    pub(crate) fn contains_file(&self, file: FileId) -> bool {
        self.included_files.get(file.0 as usize) == Some(&true)
    }

    pub fn add_record(
        &mut self,
        file: &FileEntry,
        id: RecordId,
        start: u64,
        end: u64,
    ) -> Result<RecordSlot, String> {
        if self.included_files.get(file.id.0 as usize) != Some(&true) {
            return Err("file is not in collection".into());
        }
        let range = ByteRange::checked(start, end, file.revision.size)?;
        if range.start == range.end {
            return Err("record range must be nonempty".into());
        }
        self.ids.insert(id.clone())?;
        let slot = RecordSlot(u32::try_from(self.records.len()).map_err(|_| "too many records")?);
        self.records.push(RecordEntry {
            slot,
            id,
            file: file.id,
            range,
            revision: file.revision,
            class: RecordClass::Logical,
            diagnostic_ref: DiagnosticRef(0),
        });
        Ok(slot)
    }

    pub fn add_raw_fallback(
        &mut self,
        file: &FileEntry,
        id: RecordId,
        range: ByteRange,
        diagnostic_ref: DiagnosticRef,
    ) -> Result<RecordSlot, String> {
        if self.included_files.get(file.id.0 as usize) != Some(&true)
            || diagnostic_ref == DiagnosticRef(0)
        {
            return Err("fallback file or diagnostic missing".into());
        }
        if range.end > file.revision.size
            || range.start > range.end
            || (range.start == range.end && !(range.start == 0 && file.revision.size == 0))
        {
            return Err("invalid fallback range".into());
        }
        self.ids.insert(id.clone())?;
        let slot = RecordSlot(u32::try_from(self.records.len()).map_err(|_| "too many records")?);
        self.records.push(RecordEntry {
            slot,
            id,
            file: file.id,
            range,
            revision: file.revision,
            class: RecordClass::RawFallback,
            diagnostic_ref,
        });
        Ok(slot)
    }

    pub fn finish(self, inventory: &FileInventory) -> Result<CollectionGeneration, String> {
        let file_count = u32::try_from(self.included_files.len()).map_err(|_| "too many files")?;
        if self.records.is_empty() {
            let result = CollectionGeneration {
                collection: self.collection,
                name: self.name,
                generation: self.generation,
                files: self.files,
                records: self.records,
                file_records: FileRecordIndex {
                    file_count,
                    offsets: Vec::new(),
                    slots: Vec::new(),
                },
            };
            result.validate(inventory)?;
            return Ok(result);
        }
        let mut cursors = vec![0u32; self.included_files.len()];
        for entry in &self.records {
            let count = cursors
                .get_mut(entry.file.0 as usize)
                .ok_or("file ID out of bounds")?;
            *count = count.checked_add(1).ok_or("too many records")?;
        }
        let mut offsets = Vec::with_capacity(cursors.len() + 1);
        offsets.push(0u32);
        for count in &cursors {
            offsets.push(
                (*offsets.last().unwrap())
                    .checked_add(*count)
                    .ok_or("too many records")?,
            );
        }
        let mut slots = vec![RecordSlot(0); self.records.len()];
        for (file, cursor) in cursors.iter_mut().enumerate() {
            *cursor = offsets[file];
        }
        for entry in &self.records {
            let cursor = &mut cursors[entry.file.0 as usize];
            slots[*cursor as usize] = entry.slot;
            *cursor += 1;
        }
        let result = CollectionGeneration {
            collection: self.collection,
            name: self.name,
            generation: self.generation,
            files: self.files,
            records: self.records,
            file_records: FileRecordIndex {
                file_count,
                offsets,
                slots,
            },
        };
        result.validate(inventory)?;
        Ok(result)
    }
}

#[cfg(test)]
#[path = "inventory/benchmark.rs"]
mod benchmark;

#[cfg(test)]
#[path = "inventory/tests.rs"]
mod tests;
