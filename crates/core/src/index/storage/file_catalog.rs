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
