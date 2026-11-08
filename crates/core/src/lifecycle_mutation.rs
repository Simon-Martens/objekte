//! Public file mutations backed by the lifecycle preparation and publication
//! path. Source files remain owned by the application.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use crate::build::{
    BytesPreparationError, FilePreparationBinding, PreparedFileBuild, SourceFileInput,
    prepare_file_build_from_bytes,
};
use crate::discovery::selects_file;
use crate::inventory::SourceRevision;
use crate::lifecycle::{
    FileHandle, Index, IndexError, IndexRevision, IndexView, PreparedReplacement,
};
use crate::model::CollectionId;

const MAX_PUBLICATION_RETRIES: usize = 8;


/// The change actually published by a file mutation. `Unchanged` keeps both
/// the file version and index revision intact.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileMutationKind {
    Added,
    Replaced,
    Moved,
    Unchanged,
}


/// Physical index maintenance selected for this operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileMutationStrategy {
    AppendReplacement,
    Unchanged,
}


/// One atomic file mutation, across every selected collection. The result's
/// handle is the version callers should pass to the next update or deletion.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileMutationResult {
    pub revision: IndexRevision,
    pub file: FileHandle,
    pub previous_records: u64,
    pub retired_records: u64,
    pub indexed_records: u64,
    pub affected_collections: Vec<String>,
    pub kind: FileMutationKind,
    pub strategy: FileMutationStrategy,
}


fn collection_names(
    view: &IndexView,
    previous: &[CollectionId],
    prepared: &PreparedFileBuild,
) -> Vec<String> {
    let mut ids = BTreeSet::new();
    ids.extend(previous.iter().copied());
    ids.extend(
        prepared
            .collections
            .iter()
            .map(|collection| collection.collection),
    );
    ids.into_iter()
        .map(|id| {
            view.repository().collections()[id.0 as usize]
                .definition
                .name
                .clone()
        })
        .collect()
}

fn relative_path(view: &IndexView, path: &Path) -> Result<PathBuf, IndexError> {
    let root = &view.repository().files().root;
    let relative = if path.is_absolute() {
        path.strip_prefix(root)
            .map_err(|_| IndexError::SourceUnavailable("file is outside root".into()))?
    } else {
        path
    };
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(IndexError::SourceUnavailable(
            "file path must be repository-relative and contained".into(),
        ));
    }
    Ok(relative.to_owned())
}

fn prepare_replacement_bytes(
    index: &Index,
    handle: FileHandle,
    path: &Path,
    bytes: &[u8],
    revision: SourceRevision,
) -> Result<PreparedReplacement, IndexError> {
    if handle.instance_id != index.instance_id() {
        return Err(IndexError::ForeignInstance);
    }
    let view = index.view()?;
    let actual = view
        .file_version_including_inactive(handle.file_id)
        .ok_or(IndexError::UnknownFile)?;
    if actual != handle.version {
        return Err(IndexError::FileConflict {
            file: handle.file_id,
            expected: handle.version,
            actual,
        });
    }
    let previous = view
        .file_entry(handle.file_id)
        .ok_or(IndexError::UnknownFile)?;
    let relative = relative_path(&view, path)?;
    if view
        .file_handle_for_path(&relative)
        .is_some_and(|other| other.file_id != handle.file_id)
    {
        return Err(IndexError::DestinationExists);
    }
    let repository = view.repository();
    let root = &repository.files().root;
    let mut bindings = Vec::new();
    for collection in repository.collections() {
        if selects_file(
            root,
            &collection.definition.source_path,
            &collection.definition.discovery,
            &collection.source,
            &relative,
        )
        .map_err(IndexError::Build)?
        {
            bindings.push(FilePreparationBinding {
                collection: collection.generation.collection,
                definition: &collection.definition,
                source: &collection.source,
            });
        }
    }
    if bindings.is_empty() {
        return Err(IndexError::IncompatibleConfiguration(
            "no configured collection selects this file".into(),
        ));
    }
    let input = SourceFileInput {
        id: handle.file_id,
        relative_path: relative,
        revision,
        created_ns: previous.created_ns,
    };
    let options = repository.options();
    let file = prepare_file_build_from_bytes(
        root,
        input,
        bytes,
        &bindings,
        options.bigram_alphabet,
        options.inference,
        1,
    )
    .map_err(|error| match error {
        BytesPreparationError::RevisionSizeMismatch { declared, actual } => {
            IndexError::SourceUnavailable(format!(
                "declared source size {declared} differs from supplied {actual} bytes"
            ))
        }
        BytesPreparationError::UnsupportedReader { source_id, reason } => {
            IndexError::Extension(format!("{source_id}: {reason}"))
        }
        BytesPreparationError::Build(message) => IndexError::Build(message),
    })?;
    Ok(PreparedReplacement {
        expected_revision: view.revision(),
        expected_handle: handle,
        configuration_identity: view.configuration_identity(),
        file,
    })
}


impl Index {
    /// Index one source file already created by the application. An active
    /// destination is a collision; no source bytes are written by this call.
    pub fn add_file(&self, path: &Path) -> Result<FileMutationResult, IndexError> {
        let mut conflict = None;
        for _ in 0..MAX_PUBLICATION_RETRIES {
            let prepared = self.prepare_add_file(path)?;
            let view = self.view()?;
            if view.revision() != prepared.expected_revision {
                conflict = Some(IndexError::RevisionConflict {
                    expected: prepared.expected_revision,
                    actual: view.revision(),
                });
                continue;
            }
            let file_id = prepared.file.source.id;
            let indexed_records = prepared.file.record_count();
            let affected_collections = collection_names(&view, &[], &prepared.file);
            let expected = prepared.expected_revision;
            let next = match self.stage_prepared_add(prepared) {
                Ok(next) => next,
                Err(error @ IndexError::RevisionConflict { .. }) => {
                    conflict = Some(error);
                    continue;
                }
                Err(error) => return Err(error),
            };
            let file =
                FileHandle {
                    instance_id: self.instance_id(),
                    file_id,
                    version: next.file_versions.get(file_id).ok_or(
                        IndexError::InvalidPublication("new file version missing".into()),
                    )?,
                };
            match self.publish_prepared(expected, next) {
                Ok(revision) => {
                    return Ok(FileMutationResult {
                        revision,
                        file,
                        previous_records: 0,
                        retired_records: 0,
                        indexed_records,
                        affected_collections,
                        kind: FileMutationKind::Added,
                        strategy: FileMutationStrategy::AppendReplacement,
                    });
                }
                Err(error @ IndexError::RevisionConflict { .. }) => conflict = Some(error),
                Err(error) => return Err(error),
            }
        }
        Err(conflict.expect("publication retry always captures a conflict"))
    }

    /// Reindex the current source path after the application changes it.
    /// The caller must pass the latest file handle for that file.
    pub fn update_file(
        &self,
        handle: FileHandle,
        path: &Path,
    ) -> Result<FileMutationResult, IndexError> {
        self.replace_file(handle, FileMutationKind::Replaced, |index| {
            index.prepare_replace_file(handle, path)
        })
    }

    /// Reindex a file at its new path after the application moves it. This
    /// handles path-derived identity and collection/reader changes as well as
    /// moves within a collection.
    pub fn move_file(
        &self,
        handle: FileHandle,
        destination: &Path,
    ) -> Result<FileMutationResult, IndexError> {
        self.replace_file(handle, FileMutationKind::Moved, |index| {
            index.prepare_replace_file(handle, destination)
        })
    }

    /// Index complete caller-supplied replacement bytes with their declared
    /// source revision. The caller remains responsible for source-file state;
    /// later materialization reads the file and checks that revision.
    pub fn update_file_from_bytes(
        &self,
        handle: FileHandle,
        path: &Path,
        bytes: &[u8],
        revision: SourceRevision,
    ) -> Result<FileMutationResult, IndexError> {
        self.replace_file(handle, FileMutationKind::Replaced, |index| {
            prepare_replacement_bytes(index, handle, path, bytes, revision)
        })
    }

    fn replace_file(
        &self,
        handle: FileHandle,
        kind: FileMutationKind,
        mut prepare: impl FnMut(&Self) -> Result<PreparedReplacement, IndexError>,
    ) -> Result<FileMutationResult, IndexError> {
        let mut conflict = None;
        for _ in 0..MAX_PUBLICATION_RETRIES {
            let prepared = prepare(self)?;
            let view = self.view()?;
            if view.revision() != prepared.expected_revision {
                conflict = Some(IndexError::RevisionConflict {
                    expected: prepared.expected_revision,
                    actual: view.revision(),
                });
                continue;
            }
            let previous = view
                .file_entry(handle.file_id)
                .ok_or(IndexError::UnknownFile)?;
            let retired_records = view
                .file_record_count(handle.file_id)
                .ok_or(IndexError::UnknownFile)?;
            let indexed_records = prepared.file.record_count();
            let affected_collections =
                collection_names(&view, &previous.collections, &prepared.file);
            let same_memberships = previous.collections.len() == prepared.file.collections.len()
                && previous.collections.iter().all(|collection| {
                    prepared
                        .file
                        .collections
                        .iter()
                        .any(|built| built.collection == *collection)
                });
            let unchanged = previous.relative_path == prepared.file.source.relative_path
                && previous.revision == prepared.file.source.revision
                && previous.created_ns == prepared.file.source.created_ns
                && same_memberships
                && view.file_configuration(handle.file_id) == Some(prepared.configuration_identity)
                && prepared.file.fingerprint.is_some()
                && view.file_fingerprint(handle.file_id) == prepared.file.fingerprint;
            if unchanged {
                if self.view()?.revision() != view.revision() {
                    continue;
                }
                return Ok(FileMutationResult {
                    revision: view.revision(),
                    file: handle,
                    previous_records: retired_records,
                    retired_records: 0,
                    indexed_records: retired_records,
                    affected_collections,
                    kind: FileMutationKind::Unchanged,
                    strategy: FileMutationStrategy::Unchanged,
                });
            }
            let expected = prepared.expected_revision;
            let next = match self.stage_prepared_replace(prepared) {
                Ok(next) => next,
                Err(error @ IndexError::RevisionConflict { .. }) => {
                    conflict = Some(error);
                    continue;
                }
                Err(error) => return Err(error),
            };
            let file = FileHandle {
                instance_id: self.instance_id(),
                file_id: handle.file_id,
                version: next.file_versions.get(handle.file_id).ok_or(
                    IndexError::InvalidPublication("replacement file version missing".into()),
                )?,
            };
            match self.publish_prepared(expected, next) {
                Ok(revision) => {
                    return Ok(FileMutationResult {
                        revision,
                        file,
                        previous_records: retired_records,
                        retired_records,
                        indexed_records,
                        affected_collections,
                        kind,
                        strategy: FileMutationStrategy::AppendReplacement,
                    });
                }
                Err(error @ IndexError::RevisionConflict { .. }) => conflict = Some(error),
                Err(error) => return Err(error),
            }
        }
        Err(conflict.expect("publication retry always captures a conflict"))
    }
}


#[cfg(test)]
#[path = "lifecycle_mutation.test.rs"]
mod tests;
