//! Optional file registration, provenance, and pinned catalog reads.

use std::path::{Path, PathBuf};

use crate::inventory::{ByteRange, FileEntry, FileStatus, SourceRevision};
use crate::lifecycle::{FileCatalogueRow, IndexView};
use crate::model::FileId;
use crate::model::RecordId;
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

/// File service pinned to the same publication as its enclosing index view.
pub struct FileCatalogView<'a> {
    pub(crate) view: &'a IndexView,
}

impl<'a> FileCatalogView<'a> {
    pub fn file(&self, id: FileId) -> Result<FileCatalogueRow<'a>, LookupError> {
        self.view.catalogue_row(id).ok_or(LookupError::FileNotFound)
    }

    pub fn file_by_path(&self, path: &Path) -> Option<FileCatalogueRow<'a>> {
        let id = self.view.0.catalog.id_for(path)?;
        self.view.catalogue_row(id)
    }

    fn rows_for_ids(
        &self,
        ids: Option<std::sync::Arc<[FileId]>>,
    ) -> Result<Vec<FileCatalogueRow<'a>>, LookupError> {
        ids.ok_or_else(|| LookupError::Query("unsupported file scalar comparison".into()))?
            .iter()
            .copied()
            .map(|id| self.file(id))
            .collect()
    }

    /// Ordered file-level size lookup in the pinned catalog.
    pub fn files_by_size(
        &self,
        operator: Operator,
        size: u64,
    ) -> Result<Vec<FileCatalogueRow<'a>>, LookupError> {
        self.rows_for_ids(self.view.0.catalog.size_ids(
            operator,
            &crate::model::Number::integer(size as i128),
            usize::MAX,
        ))
    }

    /// Ordered file-level modification-time lookup, in Unix nanoseconds.
    pub fn files_by_modified(
        &self,
        operator: Operator,
        modified_ns: i128,
    ) -> Result<Vec<FileCatalogueRow<'a>>, LookupError> {
        self.rows_for_ids(
            self.view
                .0
                .catalog
                .modified_ids(operator, modified_ns, usize::MAX),
        )
    }

    /// Ordered file-level creation-time lookup, in Unix nanoseconds.
    pub fn files_by_created(
        &self,
        operator: Operator,
        created_ns: i128,
    ) -> Result<Vec<FileCatalogueRow<'a>>, LookupError> {
        self.rows_for_ids(
            self.view
                .0
                .catalog
                .created_ids(operator, created_ns, usize::MAX),
        )
    }

    /// Files whose creation time could not be observed.
    pub fn files_without_creation_time(&self) -> Vec<FileCatalogueRow<'a>> {
        self.view
            .0
            .catalog
            .unknown_created_ids(usize::MAX)
            .unwrap_or_default()
            .iter()
            .copied()
            .filter_map(|id| self.view.catalogue_row(id))
            .collect()
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
        let file = self
            .view
            .0
            .catalog
            .entry(id)
            .ok_or(LookupError::FileNotFound)?;
        let mut keys = Vec::new();
        for collection_id in &file.collections {
            let number = collection_id.0 as usize;
            let state = &self.view.0.repository.collections[number];
            if let Some(slots) = state.generation.file_records.records_for(id) {
                for &slot in slots {
                    if self.view.0.active[number].contains(slot) {
                        keys.push(RecordKey::new(
                            *collection_id,
                            state.generation.records[slot.0 as usize].id.clone(),
                        ));
                    }
                }
            }
            for segment_id in self.view.0.record_routes[number].segments_for_file(id) {
                let position = self.view.0.record_routes[number]
                    .position(*segment_id)
                    .ok_or(LookupError::Query("file segment route is stale".into()))?;
                let segment = &self.view.0.segments[number][position];
                if let Some(&file_index) = segment.index.file_index.get(&id) {
                    for &slot in &segment.index.files[file_index].records {
                        if segment.active.contains(slot) {
                            keys.push(RecordKey::new(
                                *collection_id,
                                segment.index.records[slot.0 as usize].id.clone(),
                            ));
                        }
                    }
                }
            }
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
        let collection_id = self
            .view
            .0
            .repository
            .collections
            .iter()
            .enumerate()
            .find(|(_, state)| state.definition.name == collection)
            .map(|(index, _)| crate::model::CollectionId(index as u32));
        let file = self
            .view
            .0
            .catalog
            .entry(id)
            .ok_or(LookupError::FileNotFound)?;
        let Some(collection_id) = collection_id else {
            return Ok(Vec::new());
        };
        if !file.collections.contains(&collection_id) {
            return Ok(Vec::new());
        }
        let number = collection_id.0 as usize;
        let state = &self.view.0.repository.collections[number];
        let mut records = Vec::new();
        if let Some(slots) = state.generation.file_records.records_for(id) {
            for &slot in slots {
                if self.view.0.active[number].contains(slot) {
                    records.push(state.generation.records[slot.0 as usize].id.clone());
                }
            }
        }
        for segment_id in self.view.0.record_routes[number].segments_for_file(id) {
            let position = self.view.0.record_routes[number]
                .position(*segment_id)
                .ok_or(LookupError::Query("file segment route is stale".into()))?;
            let segment = &self.view.0.segments[number][position];
            if let Some(&file_index) = segment.index.file_index.get(&id) {
                for &slot in &segment.index.files[file_index].records {
                    if segment.active.contains(slot) {
                        records.push(segment.index.records[slot.0 as usize].id.clone());
                    }
                }
            }
        }
        records.sort();
        records.dedup();
        Ok(records)
    }
}
