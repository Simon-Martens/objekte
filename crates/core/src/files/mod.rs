//! Optional file registration, provenance, and pinned catalog reads.

use std::path::{Path, PathBuf};

use crate::index::storage::file_catalog::{FileCatalogReader, FileScalarPredicate};
use crate::index::storage::transfer::StorageError;
use crate::inventory::{ByteRange, FileEntry, FileStatus, SourceRevision};
use crate::lifecycle::{FileCatalogueRow, IndexView};
use crate::model::RecordId;
use crate::model::{CollectionId, FileId};
use crate::query::Operator;
use crate::query::options::ReturnMode;
use crate::records::RecordKey;

/// File origin resolved when inspecting a record, rather than stored as a
/// copied path in every record header.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordFileInfo {
    pub file_id: FileId,
    pub path: PathBuf,
    pub revision: SourceRevision,
    pub created_ns: Option<i128>,
    pub status: FileStatus,
    /// Half-open byte range. `None` is a path-only association.
    pub range: Option<ByteRange>,
}

impl RecordFileInfo {
    pub(crate) fn from_entry(file: &FileEntry, range: Option<ByteRange>) -> Self {
        Self {
            file_id: file.id,
            path: file.relative_path.to_path_buf(),
            revision: file.revision,
            created_ns: file.created_ns,
            status: file.status,
            range,
        }
    }
}

/// Explicit file service configuration for programmatic indexes.
#[derive(Clone, Debug)]
pub struct FileCatalog {
    root: PathBuf,
}

impl FileCatalog {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, std::io::Error> {
        let root = root.into().canonicalize()?;
        if !root.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotADirectory,
                "file catalog root is not a directory",
            ));
        }
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn into_root(self) -> PathBuf {
        self.root
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LookupError {
    RecordNotFound,
    FileNotFound,
    Query(String),
}

impl std::fmt::Display for LookupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RecordNotFound => f.write_str("record not found"),
            Self::FileNotFound => f.write_str("file not found in this view"),
            Self::Query(message) => message.fmt(f),
        }
    }
}

impl std::error::Error for LookupError {}

impl From<StorageError> for LookupError {
    fn from(error: StorageError) -> Self {
        Self::Query(error.to_string())
    }
}

/// File service pinned to the same publication as its enclosing index view.
pub struct FileCatalogView<'a> {
    pub(crate) view: &'a IndexView,
    pub(crate) reader: &'a dyn FileCatalogReader,
}

impl<'a> FileCatalogView<'a> {
    pub fn file(&self, id: FileId) -> Result<FileCatalogueRow<'a>, LookupError> {
        self.reader
            .file(id)?
            .map(FileCatalogueRow::from_entry)
            .ok_or(LookupError::FileNotFound)
    }

    pub fn file_by_path(&self, path: &Path) -> Result<Option<FileCatalogueRow<'a>>, LookupError> {
        Ok(self
            .reader
            .file_by_path(path)?
            .map(FileCatalogueRow::from_entry))
    }

    fn files_matching(
        &self,
        predicate: FileScalarPredicate<'_>,
    ) -> Result<Vec<FileCatalogueRow<'a>>, LookupError> {
        let ids = self
            .reader
            .scalar_candidates(predicate, usize::MAX)?
            .ok_or_else(|| LookupError::Query("unsupported file scalar comparison".into()))?;
        ids.iter().map(|&id| self.file(id)).collect()
    }

    /// Ordered file-level size lookup in the pinned catalog.
    pub fn files_by_size(
        &self,
        operator: Operator,
        size: u64,
    ) -> Result<Vec<FileCatalogueRow<'a>>, LookupError> {
        self.files_matching(FileScalarPredicate::Size(
            operator,
            &crate::model::Number::integer(size as i128),
        ))
    }

    /// Ordered file-level modification-time lookup, in Unix nanoseconds.
    pub fn files_by_modified(
        &self,
        operator: Operator,
        modified_ns: i128,
    ) -> Result<Vec<FileCatalogueRow<'a>>, LookupError> {
        self.files_matching(FileScalarPredicate::Modified(operator, modified_ns))
    }

    /// Ordered file-level creation-time lookup, in Unix nanoseconds.
    pub fn files_by_created(
        &self,
        operator: Operator,
        created_ns: i128,
    ) -> Result<Vec<FileCatalogueRow<'a>>, LookupError> {
        self.files_matching(FileScalarPredicate::Created(operator, created_ns))
    }

    /// Files whose creation time could not be observed.
    pub fn files_without_creation_time(&self) -> Result<Vec<FileCatalogueRow<'a>>, LookupError> {
        self.files_matching(FileScalarPredicate::UnknownCreated)
    }

    pub fn file_for_record(
        &self,
        collection: &str,
        id: &str,
    ) -> Result<Option<RecordFileInfo>, LookupError> {
        let hit = self
            .view
            .get_record(collection, id, ReturnMode::Locator)
            .map_err(LookupError::Query)?
            .ok_or(LookupError::RecordNotFound)?;
        Ok(hit.file)
    }

    /// Return live keys for a known file, using the base file-slot table and
    /// each participating segment's local file slot list.
    pub fn records_for_file(&self, id: FileId) -> Result<Vec<RecordKey>, LookupError> {
        let file = self.reader.file(id)?.ok_or(LookupError::FileNotFound)?;
        let mut keys = Vec::new();
        for &collection in &file.collections {
            self.visit_live_file_records(id, collection, |record| {
                keys.push(RecordKey::new(collection, record.clone()));
            })?;
        }
        keys.sort();
        keys.dedup();
        Ok(keys)
    }

    pub fn records_for_file_in_collection(
        &self,
        id: FileId,
        collection: &str,
    ) -> Result<Vec<RecordId>, LookupError> {
        let file = self.reader.file(id)?.ok_or(LookupError::FileNotFound)?;
        let repository = &self.view.0.repository;
        let Some(collection) = file.collections.iter().copied().find(|id| {
            repository
                .collections()
                .get(id.0 as usize)
                .is_some_and(|state| state.definition.name == collection)
        }) else {
            return Ok(Vec::new());
        };
        let mut records = Vec::new();
        self.visit_live_file_records(id, collection, |record| records.push(record.clone()))?;
        records.sort();
        records.dedup();
        Ok(records)
    }

    fn visit_live_file_records(
        &self,
        id: FileId,
        collection: CollectionId,
        mut visit: impl FnMut(&RecordId),
    ) -> Result<(), LookupError> {
        let number = collection.0 as usize;
        let state = self
            .view
            .0
            .repository
            .collections()
            .get(number)
            .ok_or_else(|| LookupError::Query("file points to a missing collection".into()))?;
        state.visit_base_file_slots(id, &mut |slots| {
            for &slot in slots {
                if self.view.0.active[number].contains(slot) {
                    let record = state.base_record(slot)?;
                    visit(&record.id);
                }
            }
            Ok(())
        })?;
        self.view
            .visit_segment_file_slots(id, collection, &mut |reader, active, slots| {
                for &slot in slots {
                    if active.contains(slot) {
                        let header = reader
                            .header(slot)?
                            .filter(|header| header.slot == slot)
                            .ok_or_else(|| {
                                StorageError::invalid("read file records", "invalid routed record")
                            })?;
                        visit(&header.id);
                    }
                }
                Ok(())
            })
            .map_err(|error| LookupError::Query(error.to_string()))
    }
}
