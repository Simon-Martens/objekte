//! Logical file-catalog rows used by source-free maintenance.

use std::borrow::Cow;
use std::num::NonZeroUsize;
use std::ops::Bound;
use std::path::Path;
use std::sync::Arc;

use crate::inventory::{FileEntry, SourceDiagnostic};
use crate::model::{CollectionId, FileId, Number};
use crate::query::Operator;

use super::transfer::{BatchLimit, BatchStatus, StorageError, StorageResult};

pub(crate) const FILE_TRANSFER_LIMIT: BatchLimit = BatchLimit::new(
    NonZeroUsize::new(256).unwrap(),
    NonZeroUsize::new(16 * 1024 * 1024).unwrap(),
);

/// Live metadata in increasing allocated-ID order. The allocation high-water
/// mark carries holes, including trailing retired IDs, without streaming them.
#[derive(Clone)]
pub(crate) struct FileTransferRow<'a> {
    pub(crate) entry: Cow<'a, FileEntry>,
    pub(crate) fingerprint: Option<[u8; 32]>,
    pub(crate) configuration: Option<[u8; 32]>,
}

impl FileTransferRow<'_> {
    pub(crate) fn bytes(&self) -> usize {
        let file = &self.entry;
        let fixed_bytes = std::mem::size_of::<FileEntry>() + std::mem::size_of::<Self>();
        let diagnostics_bytes = file.diagnostics.iter().fold(0usize, |bytes, diagnostic| {
            let path_bytes = diagnostic
                .field_path
                .iter()
                .flatten()
                .fold(0usize, |total, part| {
                    total.saturating_add(std::mem::size_of::<String>() + part.len())
                });
            bytes
                .saturating_add(std::mem::size_of::<SourceDiagnostic>())
                .saturating_add(diagnostic.source_id.len())
                .saturating_add(diagnostic.code.len())
                .saturating_add(diagnostic.message.len())
                .saturating_add(path_bytes)
        });
        fixed_bytes
            .saturating_add(file.relative_path.as_os_str().len())
            .saturating_add(std::mem::size_of_val::<[CollectionId]>(&file.collections))
            .saturating_add(diagnostics_bytes)
    }
}

pub(crate) trait FileCatalogSource: Send + Sync {
    fn high_water_mark(&self) -> u32;
    fn live_count(&self) -> u64;

    fn read_batch<'a>(
        &'a self,
        after: Option<FileId>,
        limit: BatchLimit,
        output: &mut Vec<FileTransferRow<'a>>,
    ) -> StorageResult<BatchStatus>;
}

pub(crate) fn transfer_files<E: From<StorageError>>(
    source: &dyn FileCatalogSource,
    high_water_mark: u32,
    limit: BatchLimit,
    cancelled: &dyn Fn() -> bool,
    mut write: impl FnMut(FileTransferRow<'_>) -> Result<(), E>,
) -> Result<(), E> {
    let mut after = None;
    let mut rows = Vec::new();
    let mut emitted = 0;
    loop {
        if cancelled() {
            return Err(StorageError::Cancelled.into());
        }

        let status = source.read_batch(after, limit, &mut rows)?;
        if status == BatchStatus::More && rows.is_empty() {
            return Err(
                StorageError::invalid("transfer file catalog", "empty non-final batch").into(),
            );
        }
        let mut bytes = 0usize;
        for (items, row) in rows.drain(..).enumerate() {
            let id = row.entry.id;
            if id.0 >= high_water_mark || after.is_some_and(|previous: FileId| id.0 <= previous.0) {
                return Err(StorageError::invalid(
                    "transfer file catalog",
                    format!("out-of-order or unallocated file ID {}", id.0),
                )
                .into());
            }
            let row_bytes = row.bytes();
            if !limit.accepts("transfer file catalog", items, bytes, row_bytes)? {
                return Err(StorageError::invalid(
                    "transfer file catalog",
                    "source exceeded batch limits",
                )
                .into());
            }
            bytes += row_bytes;
            write(row)?;
            after = Some(id);
            emitted += 1;
        }
        if status == BatchStatus::Done {
            if emitted != source.live_count() {
                return Err(StorageError::invalid(
                    "transfer file catalog",
                    "source did not emit its advertised number of live files",
                )
                .into());
            }
            return Ok(());
        }
    }
}

pub(crate) type FilePathBounds = (Bound<Arc<str>>, Bound<Arc<str>>);

pub(crate) enum FileScalarPredicate<'a> {
    Size(Operator, &'a Number),
    Modified(Operator, i128),
    Created(Operator, i128),
    UnknownCreated,
}

/// Metadata access from one published file catalog. Bounded selections
/// return absence when the operation would exceed its row allowance.
pub(crate) trait FileCatalogReader: FileCatalogSource {
    fn repository_root(&self) -> Option<&Path>;

    fn file(&self, id: FileId) -> StorageResult<Option<Cow<'_, FileEntry>>>;

    fn id_for_path(&self, path: &Path) -> StorageResult<Option<FileId>>;

    fn path_candidates(
        &self,
        bounds: FilePathBounds,
        limit: usize,
    ) -> StorageResult<Option<Vec<FileId>>>;

    /// Visit an exact path range in bounded batches.
    fn visit_paths(
        &self,
        bounds: FilePathBounds,
        visit: &mut dyn FnMut(&[FileId]) -> StorageResult<()>,
    ) -> StorageResult<()>;

    fn scalar_candidates(
        &self,
        predicate: FileScalarPredicate<'_>,
        limit: usize,
    ) -> StorageResult<Option<Arc<[FileId]>>>;
}

/// The discovered base is ordered by allocated ID and includes final
/// fingerprints once initial construction is complete.
pub(crate) trait FileCatalogBase: FileCatalogSource + std::any::Any {
    fn root(&self) -> Option<&Path>;
    fn configuration(&self) -> [u8; 32];
    fn collection_count(&self) -> usize;
}

pub(crate) type FileCatalogHandle = Arc<dyn FileCatalogRevision>;

pub(crate) trait FileCatalogRevision: FileCatalogReader {
    fn construction_complete(&self) -> bool;
    fn estimates_delta_bytes(&self) -> usize;
    fn fingerprint(&self, id: FileId) -> StorageResult<Option<[u8; 32]>>;
    fn configuration(&self, id: FileId) -> StorageResult<Option<[u8; 32]>>;
    fn edit(&self) -> StorageResult<Box<dyn FileCatalogEditor>>;
}

pub(crate) enum FileCatalogDelta<'a> {
    Append {
        entry: FileEntry,
        fingerprint: Option<[u8; 32]>,
        configuration: Option<[u8; 32]>,
    },
    Replace {
        entry: FileEntry,
        fingerprint: Option<[u8; 32]>,
        configuration: Option<[u8; 32]>,
    },
    Retire(FileId),
    Rebase(&'a dyn FileCatalogBase),
    ConstructionComplete(bool),
}

pub(crate) trait FileCatalogEditor: Send {
    fn apply(&mut self, delta: FileCatalogDelta<'_>) -> StorageResult<()>;
    fn seal(self: Box<Self>) -> StorageResult<FileCatalogHandle>;
}

pub(crate) fn edit_file_catalog<'a>(
    catalog: &dyn FileCatalogRevision,
    changes: impl IntoIterator<Item = FileCatalogDelta<'a>>,
) -> StorageResult<FileCatalogHandle> {
    let mut editor = catalog.edit()?;
    for change in changes {
        editor.apply(change)?;
    }
    editor.seal()
}

impl FileCatalogSource for FileCatalogHandle {
    fn high_water_mark(&self) -> u32 {
        self.as_ref().high_water_mark()
    }

    fn live_count(&self) -> u64 {
        self.as_ref().live_count()
    }

    fn read_batch<'a>(
        &'a self,
        after: Option<FileId>,
        limit: BatchLimit,
        output: &mut Vec<FileTransferRow<'a>>,
    ) -> StorageResult<BatchStatus> {
        self.as_ref().read_batch(after, limit, output)
    }
}

impl FileCatalogReader for FileCatalogHandle {
    fn repository_root(&self) -> Option<&Path> {
        self.as_ref().repository_root()
    }

    fn file(&self, id: FileId) -> StorageResult<Option<Cow<'_, FileEntry>>> {
        self.as_ref().file(id)
    }

    fn id_for_path(&self, path: &Path) -> StorageResult<Option<FileId>> {
        self.as_ref().id_for_path(path)
    }

    fn path_candidates(
        &self,
        bounds: FilePathBounds,
        limit: usize,
    ) -> StorageResult<Option<Vec<FileId>>> {
        self.as_ref().path_candidates(bounds, limit)
    }

    fn visit_paths(
        &self,
        bounds: FilePathBounds,
        visit: &mut dyn FnMut(&[FileId]) -> StorageResult<()>,
    ) -> StorageResult<()> {
        self.as_ref().visit_paths(bounds, visit)
    }

    fn scalar_candidates(
        &self,
        predicate: FileScalarPredicate<'_>,
        limit: usize,
    ) -> StorageResult<Option<Arc<[FileId]>>> {
        self.as_ref().scalar_candidates(predicate, limit)
    }
}

impl dyn FileCatalogRevision {
    pub(crate) fn allocated_files(&self) -> usize {
        self.high_water_mark() as usize
    }

    pub(crate) fn is_active(&self, id: FileId) -> StorageResult<bool> {
        Ok(self.file(id)?.is_some())
    }

    pub(crate) fn appended(
        &self,
        entry: FileEntry,
        fingerprint: Option<[u8; 32]>,
        configuration: Option<[u8; 32]>,
    ) -> StorageResult<FileCatalogHandle> {
        edit_file_catalog(
            self,
            [FileCatalogDelta::Append {
                entry,
                fingerprint,
                configuration,
            }],
        )
    }

    pub(crate) fn replaced(
        &self,
        entry: FileEntry,
        fingerprint: Option<[u8; 32]>,
        configuration: Option<[u8; 32]>,
    ) -> StorageResult<FileCatalogHandle> {
        edit_file_catalog(
            self,
            [FileCatalogDelta::Replace {
                entry,
                fingerprint,
                configuration,
            }],
        )
    }

    pub(crate) fn retired(&self, id: FileId) -> StorageResult<FileCatalogHandle> {
        edit_file_catalog(self, [FileCatalogDelta::Retire(id)])
    }
}

/// Incremental metadata enumeration tied to one pinned file catalog.
pub(crate) struct FileRows<'a> {
    source: &'a dyn FileCatalogSource,
    batch: Vec<FileTransferRow<'a>>,
    after: Option<FileId>,
    count: u64,
    done: bool,
}

impl<'a> FileRows<'a> {
    pub(crate) fn new(source: &'a dyn FileCatalogSource) -> Self {
        Self {
            source,
            batch: Vec::new(),
            after: None,
            count: 0,
            done: false,
        }
    }

    fn refill(&mut self) -> StorageResult<()> {
        const OP: &str = "enumerate file metadata";
        let status = self
            .source
            .read_batch(self.after, FILE_TRANSFER_LIMIT, &mut self.batch)?;
        if status == BatchStatus::More && self.batch.is_empty() {
            return Err(StorageError::invalid(OP, "file cursor made no progress"));
        }
        let mut bytes = 0;
        let allocated = self.source.high_water_mark();
        for (items, row) in self.batch.iter().enumerate() {
            let id = row.entry.id;
            let size = row.bytes();
            if id.0 >= allocated || self.after.is_some_and(|after| id.0 <= after.0) {
                return Err(StorageError::invalid(
                    OP,
                    "file cursor returned an invalid ID",
                ));
            }
            if !FILE_TRANSFER_LIMIT.accepts(OP, items, bytes, size)? {
                return Err(StorageError::invalid(
                    OP,
                    "file cursor exceeded the batch limit",
                ));
            }
            self.after = Some(id);
            self.count += 1;
            bytes += size;
        }
        self.done = status == BatchStatus::Done;
        if self.done && self.count != self.source.live_count() {
            return Err(StorageError::invalid(
                OP,
                "file cursor returned another live row count",
            ));
        }
        self.batch.reverse();
        Ok(())
    }
}

impl<'a> Iterator for FileRows<'a> {
    type Item = StorageResult<FileTransferRow<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.batch.is_empty() && !self.done {
            if let Err(error) = self.refill() {
                self.done = true;
                self.batch.clear();
                return Some(Err(error));
            }
        }
        self.batch.pop().map(Ok)
    }
}
