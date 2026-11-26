//! Public file mutations backed by the lifecycle preparation and publication
//! path. Source files remain owned by the application.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use crate::construction::file_preparation::{PreparedFileBuild, verify_current_source};
use crate::inventory::SourceRevision;
use crate::inventory::discovery::selects_file;
use crate::lifecycle::maintenance::compaction::compact_published;
use crate::lifecycle::mutation::preparation::PreparedReplacement;
use crate::lifecycle::mutation::verified::{
    VerificationError, VerificationResult, preserve_partition_record_slots,
    preserve_single_record_slots, remove_verified_old_shards, verify_old_content,
    verify_old_content_with_layout,
};
use crate::lifecycle::{FileHandle, Index, IndexError, IndexRevision, IndexView, PublishedIndex};
use crate::model::{CollectionId, FileId, MetadataField, PathRoot};
use crate::repository::configuration::IdentityPolicy;

use preparation::{created_ns, prepare_replacement_bytes};
use staging::IndexedFilePlacement;

const MAX_PUBLICATION_RETRIES: usize = 8;

fn verify_file_source(view: &IndexView, file: &PreparedFileBuild) -> Result<(), IndexError> {
    #[cfg(test)]
    let identity = if tests::reach_file_preparation_checkpoint("force-digest") {
        tests::reach_file_preparation_checkpoint("digest-read");
        None
    } else {
        file.identity
    };
    #[cfg(not(test))]
    let identity = file.identity;
    verify_current_source(
        view.repository().required_file_root(),
        &file.source,
        identity,
        file.fingerprint,
        file.reader(view.repository()).map(|source| source.reader()),
    )
    .map_err(IndexError::SourceUnavailable)
}

fn compact_verified_base(next: &PublishedIndex) -> Result<PublishedIndex, VerificationError> {
    compact_published(next, next.revision).map_err(|error| match error {
        IndexError::ResourceLimit(_) => VerifiedFallbackReason::CompactionBudget.into(),
        IndexError::Storage(_) => error.into(),
        _ => VerifiedFallbackReason::CompactionUnavailable.into(),
    })
}

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
    /// The destination's raw digest and source revision matched the indexed
    /// file; only its catalog route and file version changed.
    ContentPreservingMove,
    VerifiedSlotPreserving,
    VerifiedPhysicalRemoval,
    Unchanged,
}

/// Why a requested verified operation safely used the normal tombstone and
/// append strategy. A wrong old-content hint is never a mutation error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VerifiedFallbackReason {
    MissingOldContent,
    MissingFingerprint,
    ConfigurationChanged,
    ContentMismatch,
    UnsupportedReader,
    ReconstructionFailed,
    IndexedLayoutChanged,
    BaseIndexUnsupported,
    CompactionBudget,
    CompactionUnavailable,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum UpdateStrategy {
    #[default]
    AppendReplacement,
    PreferVerified,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct UpdateOptions<'a> {
    pub old_content: Option<&'a [u8]>,
    pub strategy: UpdateStrategy,
}

/// Result of removing one physical source entry from the published index.
/// The source file itself is never changed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeleteStrategy {
    Tombstone,
    VerifiedPhysicalRemoval,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeleteResult {
    pub revision: IndexRevision,
    pub file: FileId,
    pub retired_records: u64,
    pub affected_collections: Vec<String>,
    pub strategy: DeleteStrategy,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedDeleteResult {
    pub deletion: DeleteResult,
    pub fallback_reason: Option<VerifiedFallbackReason>,
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
    pub fallback_reason: Option<VerifiedFallbackReason>,
}

impl Index {
    /// Retire all records owned by one file across its collections in one
    /// publication. This changes index state only, and requires no source
    /// content or source-file I/O. A later scan can add a still-present file.
    pub fn delete_file(&self, handle: FileHandle) -> Result<DeleteResult, IndexError> {
        let _writer = self.0.writer.lock().map_err(|_| IndexError::LockPoisoned)?;
        let (expected, next, retired_records, affected_collections) =
            self.stage_prepared_delete(handle)?;
        let revision = self.publish_prepared(expected, next)?;
        Ok(DeleteResult {
            revision,
            file: handle.file_id,
            retired_records,
            affected_collections,
            strategy: DeleteStrategy::Tombstone,
        })
    }
}

fn collection_names(
    view: &IndexView,
    previous: &[CollectionId],
    prepared: &[CollectionId],
) -> Vec<String> {
    let ids: BTreeSet<_> = previous.iter().chain(prepared).copied().collect();
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
    let root = view.repository().required_file_root();
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

fn path_independent_memberships(
    view: &IndexView,
    old: &crate::inventory::FileEntry,
    destination: &Path,
) -> Result<bool, IndexError> {
    if old.relative_path.extension() != destination.extension()
        || old.status != crate::inventory::FileStatus::Valid
        || !old.diagnostics.is_empty()
    {
        return Ok(false);
    }
    let repository = view.repository();
    let root = repository.required_file_root();
    for collection in repository.collections() {
        if collection.file_source().is_none() {
            continue;
        }
        let selected = selects_file(
            root,
            &collection.required_file_source().source_path,
            &collection.required_file_source().discovery,
            &collection.required_file_reader(),
            destination,
        )
        .map_err(IndexError::Build)?;
        if selected != old.collections.contains(&collection.collection_id()) {
            return Ok(false);
        }
        if selected
            && (!collection
                .required_file_reader()
                .reader()
                .supports_verified_layout()
                || !matches!(
                    collection.required_file_source().identity,
                    IdentityPolicy::Generated | IdentityPolicy::Intrinsic(_)
                )
                || collection
                    .definition
                    .sort_fields
                    .iter()
                    .any(|field| field.root == PathRoot::Metadata(MetadataField::FilePath))
                || collection
                    .definition
                    .schema
                    .composite_sorts
                    .iter()
                    .flatten()
                    .any(|field| field.field == MetadataField::FilePath.as_str()))
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn destination_has_importer(view: &IndexView, destination: &Path) -> Result<bool, IndexError> {
    let relative = relative_path(view, destination)?;
    if crate::inventory::discovery::contains_generated_directory(&relative) {
        return Err(IndexError::IncompatibleConfiguration(
            "move destination is in a generated index namespace".into(),
        ));
    }
    let root = view
        .repository()
        .repository_root()
        .ok_or_else(|| IndexError::SourceUnavailable("file catalog is not registered".into()))?;
    for collection in view.repository().collections() {
        let Some(source) = collection.file_source() else {
            continue;
        };
        if selects_file(
            root,
            &source.source_path,
            &source.discovery,
            &collection.required_file_reader(),
            &relative,
        )
        .map_err(IndexError::Build)?
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn hash_destination(
    path: &Path,
    backend: &dyn crate::source::SourceReader,
) -> Result<([u8; 32], SourceRevision, Option<i128>), IndexError> {
    let mut source =
        std::fs::File::open(path).map_err(|error| IndexError::Io(error.to_string()))?;
    let before = source
        .metadata()
        .map_err(|error| IndexError::Io(error.to_string()))?;
    if !before.is_file() {
        return Err(IndexError::SourceUnavailable(
            "move destination is not a file".into(),
        ));
    }
    let revision = SourceRevision::from_metadata(&before).map_err(IndexError::Io)?;
    let creation = created_ns(&before);
    let digest = backend
        .fingerprint(&mut source)
        .map_err(|error| IndexError::Io(error.to_string()))?;
    let after = source
        .metadata()
        .map_err(|error| IndexError::Io(error.to_string()))?;
    let path_after = std::fs::metadata(path).map_err(|error| IndexError::Io(error.to_string()))?;
    if SourceRevision::from_metadata(&after).map_err(IndexError::Io)? != revision
        || SourceRevision::from_metadata(&path_after).map_err(IndexError::Io)? != revision
        || created_ns(&after) != creation
        || created_ns(&path_after) != creation
    {
        return Err(IndexError::SourceUnavailable(
            "move destination changed during hashing".into(),
        ));
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if before.dev() != path_after.dev() || before.ino() != path_after.ino() {
            return Err(IndexError::SourceUnavailable(
                "move destination changed during hashing".into(),
            ));
        }
    }
    Ok((digest, revision, creation))
}

impl Index {
    /// Index one source file already created by the application. An active
    /// destination is a collision; no source bytes are written by this call.
    pub fn add_file(&self, path: &Path) -> Result<FileMutationResult, IndexError> {
        let mut conflict = None;
        let mut ready = None;
        for _ in 0..MAX_PUBLICATION_RETRIES {
            let (mut prepared, placement, indexed_records) = match ready.take() {
                Some(ready) => ready,
                None => {
                    let mut prepared = self.prepare_add_file(path)?;
                    let indexed_records = prepared.file.record_count();
                    let prepared_view = self.view()?;
                    if prepared_view.configuration_identity() != prepared.configuration_identity
                        || prepared_view.0.catalog.allocated_files()
                            != prepared.file.source.id.0 as usize
                    {
                        conflict = Some(IndexError::RevisionConflict {
                            expected: prepared.expected_revision,
                            actual: prepared_view.revision(),
                        });
                        continue;
                    }
                    let placement =
                        IndexedFilePlacement::prepare(&prepared_view, &mut prepared.file)?;
                    (prepared, placement, indexed_records)
                }
            };
            let view = self.view()?;
            if view.revision() != prepared.expected_revision {
                if view.configuration_identity() == prepared.configuration_identity
                    && view.0.catalog.allocated_files() == prepared.file.source.id.0 as usize
                {
                    prepared.expected_revision = view.revision();
                } else {
                    conflict = Some(IndexError::RevisionConflict {
                        expected: prepared.expected_revision,
                        actual: view.revision(),
                    });
                    continue;
                }
            }
            let file_id = prepared.file.source.id;
            let affected_collections = collection_names(&view, &[], &placement.entry.collections);
            let expected = prepared.expected_revision;
            let next = Self::stage_file_add(&view, &prepared, &placement)?;
            let file =
                FileHandle {
                    instance_id: self.instance_id(),
                    file_id,
                    version: next.catalog.version(file_id)?.ok_or(
                        IndexError::InvalidPublication("new file version missing".into()),
                    )?,
                };
            verify_file_source(&view, &prepared.file)?;
            match self.install_file_candidate(expected, next) {
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
                        fallback_reason: None,
                    });
                }
                Err(error @ IndexError::RevisionConflict { .. }) => {
                    conflict = Some(error);
                    // Another append claims the same next file ID. Its
                    // successor needs fresh preparation for the new ID.
                    let current = self.view()?;
                    if current.configuration_identity() == prepared.configuration_identity
                        && current.0.catalog.allocated_files() == file_id.0 as usize
                    {
                        ready = Some((prepared, placement, indexed_records));
                    }
                }
                Err(error) => return Err(error),
            }
        }
        Err(conflict.expect("publication retry always captures a conflict"))
    }

    fn install_file_candidate(
        &self,
        expected: IndexRevision,
        next: PublishedIndex,
    ) -> Result<IndexRevision, IndexError> {
        let _placement = self.0.writer.lock().map_err(|_| IndexError::LockPoisoned)?;
        self.publish_prepared(expected, next)
    }

    /// Reindex the current source path after the application changes it.
    /// The caller must pass the latest file handle for that file.
    pub fn update_file(
        &self,
        handle: FileHandle,
        path: &Path,
    ) -> Result<FileMutationResult, IndexError> {
        self.update_file_with_options(handle, path, UpdateOptions::default())
    }

    /// Optionally verify caller-supplied old content for physical removal.
    /// If it cannot be verified, the update succeeds through appended
    /// replacement and reports a structured fallback reason.
    pub fn update_file_with_options(
        &self,
        handle: FileHandle,
        path: &Path,
        options: UpdateOptions<'_>,
    ) -> Result<FileMutationResult, IndexError> {
        if self.catalog_only_file(handle)? {
            return self.change_catalog_only_file(handle, path, false, false);
        }
        self.replace_file(handle, FileMutationKind::Replaced, options, |index| {
            index.prepare_replace_file(handle, path)
        })
    }

    fn catalog_only_file(&self, handle: FileHandle) -> Result<bool, IndexError> {
        if handle.instance_id != self.instance_id() {
            return Err(IndexError::ForeignInstance);
        }
        let view = self.view()?;
        let file = view
            .file_entry(handle.file_id)?
            .ok_or(IndexError::UnknownFile)?;
        Ok(view.0.imported_collections(&file)?.is_empty())
    }

    /// Refresh metadata for a registrar-declared file with no file importer.
    /// Its independently owned records survive the file content change.
    fn change_catalog_only_file(
        &self,
        handle: FileHandle,
        path: &Path,
        moving: bool,
        retire_imported: bool,
    ) -> Result<FileMutationResult, IndexError> {
        let view = self.view()?;
        let actual = view
            .file_version_including_inactive(handle.file_id)?
            .ok_or(IndexError::UnknownFile)?;
        if actual != handle.version {
            return Err(IndexError::FileConflict {
                file: handle.file_id,
                expected: handle.version,
                actual,
            });
        }
        let previous = view
            .file_entry(handle.file_id)?
            .ok_or(IndexError::UnknownFile)?;
        let root = view
            .repository()
            .repository_root()
            .ok_or(IndexError::UnknownFile)?;
        let canonical = path
            .canonicalize()
            .map_err(|error| IndexError::Io(error.to_string()))?;
        let relative = canonical
            .strip_prefix(root)
            .map_err(|_| IndexError::InvalidPublication("catalog path escapes root".into()))?;
        if !moving && relative != previous.relative_path.as_path() {
            return Err(IndexError::InvalidPublication(
                "catalog update path differs from active file".into(),
            ));
        }
        if view
            .file_handle_for_path(relative)?
            .is_some_and(|other| other.file_id != handle.file_id)
        {
            return Err(IndexError::DestinationExists);
        }
        let metadata =
            std::fs::metadata(&canonical).map_err(|error| IndexError::Io(error.to_string()))?;
        if !metadata.is_file() {
            return Err(IndexError::InvalidPublication(
                "catalog update is not a regular file".into(),
            ));
        }
        let revision = SourceRevision::from_metadata(&metadata).map_err(IndexError::Io)?;
        let creation = created_ns(&metadata);
        let source_unchanged = revision == previous.revision;
        if relative == previous.relative_path.as_path()
            && revision == previous.revision
            && creation == previous.created_ns
        {
            return Ok(FileMutationResult {
                revision: view.revision(),
                file: handle,
                previous_records: 0,
                retired_records: 0,
                indexed_records: 0,
                affected_collections: Vec::new(),
                kind: FileMutationKind::Unchanged,
                strategy: FileMutationStrategy::Unchanged,
                fallback_reason: None,
            });
        }
        let mut next = (*view.0).clone();
        let mut entry = previous.clone().into_owned();
        entry.relative_path = relative.to_path_buf().into();
        entry.revision = revision;
        entry.created_ns = creation;
        let imported = if retire_imported {
            view.0.imported_collections(&previous)?
        } else {
            std::collections::BTreeSet::new()
        };
        let retired_records = if retire_imported {
            next.retire_file_records(&previous)?
        } else {
            0
        };
        let linked = next.refresh_record_links(handle.file_id, Some(&entry))?;
        entry
            .collections
            .retain(|collection| !imported.contains(collection));
        for collection in linked {
            if !entry.collections.contains(&collection) {
                entry.collections.push(collection);
            }
        }
        next.catalog = if entry.collections.is_empty() {
            next.catalog.retired(handle.file_id)?
        } else {
            next.catalog.replaced_incarnation(entry, None, None)?
        };

        next.revision = view.revision().next()?;
        let file = FileHandle {
            instance_id: self.instance_id(),
            file_id: handle.file_id,
            version: next
                .catalog
                .version(handle.file_id)?
                .ok_or(IndexError::UnknownFile)?,
        };
        let revision = self.publish_prepared(view.revision(), next)?;
        Ok(FileMutationResult {
            revision,
            file,
            previous_records: retired_records,
            retired_records,
            indexed_records: 0,
            affected_collections: previous
                .collections
                .iter()
                .map(|id| {
                    view.repository().collections[id.0 as usize]
                        .definition
                        .name
                        .clone()
                })
                .collect(),
            kind: if moving {
                FileMutationKind::Moved
            } else {
                FileMutationKind::Replaced
            },
            strategy: if moving && source_unchanged {
                FileMutationStrategy::ContentPreservingMove
            } else {
                FileMutationStrategy::AppendReplacement
            },
            fallback_reason: None,
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
        if handle.instance_id != self.instance_id() {
            return Err(IndexError::ForeignInstance);
        }
        let view = self.view()?;
        let actual = view
            .file_version_including_inactive(handle.file_id)?
            .ok_or(IndexError::UnknownFile)?;
        if actual != handle.version {
            return Err(IndexError::FileConflict {
                file: handle.file_id,
                expected: handle.version,
                actual,
            });
        }
        let old = view
            .file_entry(handle.file_id)?
            .ok_or(IndexError::UnknownFile)?;
        let imported = !view.0.imported_collections(&old)?.is_empty();
        let selected = destination_has_importer(&view, destination)?;
        if !selected {
            if imported {
                let relative = relative_path(&view, destination)?;
                let root = view.repository().required_file_root();
                if !root.join(relative).is_file() {
                    return Err(IndexError::IncompatibleConfiguration(
                        "no configured collection selects this file".into(),
                    ));
                }
            }
            return self.change_catalog_only_file(handle, destination, true, imported);
        }
        if !imported {
            return self.replace_file(
                handle,
                FileMutationKind::Moved,
                UpdateOptions::default(),
                |index| index.prepare_replace_file(handle, destination),
            );
        }
        if let Some(moved) = self.try_content_preserving_move(handle, destination, view)? {
            return Ok(moved);
        }
        self.replace_file(
            handle,
            FileMutationKind::Moved,
            UpdateOptions::default(),
            |index| index.prepare_replace_file(handle, destination),
        )
    }

    /// Reuse content-derived contributions only after checking the destination
    /// bytes and every affected collection's path semantics. The source file
    /// has already been moved by the caller; this operation writes no source.
    fn try_content_preserving_move(
        &self,
        handle: FileHandle,
        destination: &Path,
        first_view: IndexView,
    ) -> Result<Option<FileMutationResult>, IndexError> {
        if handle.instance_id != self.instance_id() {
            return Err(IndexError::ForeignInstance);
        }
        let mut conflict = None;
        let mut first_view = Some(first_view);
        for _ in 0..MAX_PUBLICATION_RETRIES {
            let view = if let Some(view) = first_view.take() {
                view
            } else {
                self.view()?
            };
            let actual = view
                .file_version_including_inactive(handle.file_id)?
                .ok_or(IndexError::UnknownFile)?;
            if actual != handle.version {
                return Err(IndexError::FileConflict {
                    file: handle.file_id,
                    expected: handle.version,
                    actual,
                });
            }
            let old = view
                .file_entry(handle.file_id)?
                .ok_or(IndexError::UnknownFile)?;
            let relative = relative_path(&view, destination)?;
            if relative.as_path() == old.relative_path.as_path() {
                return Ok(None);
            }
            if view
                .file_handle_for_path(&relative)?
                .is_some_and(|other| other.file_id != handle.file_id)
            {
                return Err(IndexError::DestinationExists);
            }
            if !path_independent_memberships(&view, &old, &relative)? {
                return Ok(None);
            }
            let Some(fingerprint) = view.file_fingerprint(handle.file_id)? else {
                return Ok(None);
            };
            if view.file_configuration(handle.file_id)? != Some(view.configuration_identity()) {
                return Ok(None);
            }
            let absolute = view.repository().required_file_root().join(&relative);
            let Some(backend) = old.collections.iter().find_map(|id| {
                view.repository()
                    .collections()
                    .get(id.0 as usize)?
                    .source
                    .as_ref()
            }) else {
                return Ok(None);
            };
            let (digest, revision, creation) = hash_destination(&absolute, backend.reader())?;
            if digest != fingerprint || revision != old.revision || creation != old.created_ns {
                return Ok(None);
            }
            let mut next = (*view.0).clone();
            let mut relocated = old.clone().into_owned();
            relocated.relative_path = relative.into();
            next.catalog = next.catalog.replaced_incarnation(
                relocated,
                Some(fingerprint),
                Some(view.configuration_identity()),
            )?;

            next.revision = view.revision().next()?;
            let file = FileHandle {
                instance_id: self.instance_id(),
                file_id: handle.file_id,
                version: next.catalog.version(handle.file_id)?.ok_or_else(|| {
                    IndexError::InvalidPublication("moved file version missing".into())
                })?,
            };
            let indexed_records = view
                .file_record_count(handle.file_id)?
                .ok_or(IndexError::UnknownFile)?;
            let affected_collections = old
                .collections
                .iter()
                .map(|id| {
                    view.repository().collections()[id.0 as usize]
                        .definition
                        .name
                        .clone()
                })
                .collect();
            match self.publish_prepared(view.revision(), next) {
                Ok(revision) => {
                    return Ok(Some(FileMutationResult {
                        revision,
                        file,
                        previous_records: indexed_records,
                        retired_records: 0,
                        indexed_records,
                        affected_collections,
                        kind: FileMutationKind::Moved,
                        strategy: FileMutationStrategy::ContentPreservingMove,
                        fallback_reason: None,
                    }));
                }
                Err(error @ IndexError::RevisionConflict { .. }) => conflict = Some(error),
                Err(error) => return Err(error),
            }
        }
        Err(conflict.expect("publication retry always captures a conflict"))
    }

    /// Conditionally update a managed file when core's opened source has the
    /// supplied revision and bytes. The supplied bytes are never published as
    /// file content; core prepares the update from its own opened source.
    pub fn update_file_from_bytes(
        &self,
        handle: FileHandle,
        path: &Path,
        bytes: &[u8],
        revision: SourceRevision,
    ) -> Result<FileMutationResult, IndexError> {
        self.update_file_from_bytes_with_options(
            handle,
            path,
            bytes,
            revision,
            UpdateOptions::default(),
        )
    }

    pub fn update_file_from_bytes_with_options(
        &self,
        handle: FileHandle,
        path: &Path,
        bytes: &[u8],
        revision: SourceRevision,
        options: UpdateOptions<'_>,
    ) -> Result<FileMutationResult, IndexError> {
        self.replace_file(handle, FileMutationKind::Replaced, options, |index| {
            prepare_replacement_bytes(index, handle, path, bytes, revision)
        })
    }

    fn replace_file(
        &self,
        handle: FileHandle,
        kind: FileMutationKind,
        options: UpdateOptions<'_>,
        prepare: impl FnOnce(&Self) -> Result<PreparedReplacement, IndexError>,
    ) -> Result<FileMutationResult, IndexError> {
        let mut conflict = None;
        let mut prepared = prepare(self)?;
        let indexed_records = prepared.file.record_count();
        let mut placement = None::<IndexedFilePlacement>;
        let mut old_layout = None;
        for _ in 0..MAX_PUBLICATION_RETRIES {
            let view = self.view()?;
            if view.configuration_identity() != prepared.configuration_identity {
                return Err(IndexError::IncompatibleConfiguration(
                    "configuration changed during file preparation".into(),
                ));
            }
            let actual = view
                .file_version_including_inactive(handle.file_id)?
                .ok_or(IndexError::UnknownFile)?;
            if actual != handle.version {
                return Err(IndexError::FileConflict {
                    file: handle.file_id,
                    expected: handle.version,
                    actual,
                });
            }
            prepared.expected_revision = view.revision();
            let previous = view
                .file_entry(handle.file_id)?
                .ok_or(IndexError::UnknownFile)?;
            let retired_records = view
                .file_record_count(handle.file_id)?
                .ok_or(IndexError::UnknownFile)?;
            let prepared_collections;
            let memberships = if let Some(placement) = &placement {
                &placement.entry.collections
            } else {
                prepared_collections = prepared
                    .file
                    .collections
                    .iter()
                    .map(|built| built.collection)
                    .collect::<Vec<_>>();
                &prepared_collections
            };
            let affected_collections = collection_names(&view, &previous.collections, memberships);
            let same_memberships = previous.collections.len() == memberships.len()
                && previous
                    .collections
                    .iter()
                    .all(|collection| memberships.contains(collection));
            let unchanged = previous.relative_path.as_path()
                == prepared.file.source.relative_path.as_path()
                && previous.revision == prepared.file.source.revision
                && previous.created_ns == prepared.file.source.created_ns
                && same_memberships
                && view.file_configuration(handle.file_id)?
                    == Some(prepared.configuration_identity)
                && prepared.file.fingerprint.is_some()
                && view.file_fingerprint(handle.file_id)? == prepared.file.fingerprint;
            if unchanged {
                verify_file_source(&view, &prepared.file)?;
                let _placement = self.0.writer.lock().map_err(|_| IndexError::LockPoisoned)?;
                let actual = self.view()?.revision();
                if actual != view.revision() {
                    conflict = Some(IndexError::RevisionConflict {
                        expected: view.revision(),
                        actual,
                    });
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
                    fallback_reason: None,
                });
            }
            let verified = match options.strategy {
                UpdateStrategy::AppendReplacement => None,
                UpdateStrategy::PreferVerified => Some(match options.old_content {
                    Some(bytes) => {
                        verify_old_content_with_layout(&view, handle, bytes, &mut old_layout)
                            .into_fallback()?
                    }
                    None => Err(VerifiedFallbackReason::MissingOldContent),
                }),
            };
            let expected = prepared.expected_revision;
            if placement.is_none() {
                placement = Some(IndexedFilePlacement::prepare(&view, &mut prepared.file)?);
            }
            let next = self.stage_file_replace(
                &view,
                &prepared,
                placement.as_ref().expect("file placement was prepared"),
            )?;
            let (next, strategy, fallback_reason) = match verified {
                Some(Ok(plan)) => {
                    #[cfg(test)]
                    tests::reach_file_preparation_checkpoint("verified-compaction");
                    if plan.requires_base_compaction() {
                        match compact_verified_base(&next).into_fallback()? {
                            Ok(compacted) => (
                                compacted,
                                FileMutationStrategy::VerifiedPhysicalRemoval,
                                None,
                            ),
                            Err(reason) => {
                                (next, FileMutationStrategy::AppendReplacement, Some(reason))
                            }
                        }
                    } else if let Some(preserved) =
                        match preserve_single_record_slots(&view, &next, &plan, handle.file_id)? {
                            Some(candidate) => Some(candidate),
                            None => preserve_partition_record_slots(
                                &view,
                                &next,
                                &plan,
                                handle.file_id,
                            )?,
                        }
                    {
                        (
                            preserved,
                            FileMutationStrategy::VerifiedSlotPreserving,
                            None,
                        )
                    } else {
                        match remove_verified_old_shards(&next, &plan, handle.file_id)
                            .into_fallback()?
                        {
                            Ok(compacted) => (
                                compacted,
                                FileMutationStrategy::VerifiedPhysicalRemoval,
                                None,
                            ),
                            Err(reason) => {
                                (next, FileMutationStrategy::AppendReplacement, Some(reason))
                            }
                        }
                    }
                }
                Some(Err(reason)) => (next, FileMutationStrategy::AppendReplacement, Some(reason)),
                None => (next, FileMutationStrategy::AppendReplacement, None),
            };
            let file = FileHandle {
                instance_id: self.instance_id(),
                file_id: handle.file_id,
                version: next.catalog.version(handle.file_id)?.ok_or(
                    IndexError::InvalidPublication("replacement file version missing".into()),
                )?,
            };
            verify_file_source(&view, &prepared.file)?;
            match self.install_file_candidate(expected, next) {
                Ok(revision) => {
                    return Ok(FileMutationResult {
                        revision,
                        file,
                        previous_records: retired_records,
                        retired_records,
                        indexed_records,
                        affected_collections,
                        kind,
                        strategy,
                        fallback_reason,
                    });
                }
                Err(error @ IndexError::RevisionConflict { .. }) => conflict = Some(error),
                Err(error) => return Err(error),
            }
        }
        Err(conflict.expect("publication retry always captures a conflict"))
    }

    /// Delete an indexed file after optionally proving its old raw bytes.
    /// A mismatched hint safely falls back to the existing tombstone path.
    /// The application owns any physical source-file deletion.
    pub fn delete_file_with_old_content(
        &self,
        handle: FileHandle,
        old_content: &[u8],
    ) -> Result<VerifiedDeleteResult, IndexError> {
        let mut conflict = None;
        for _ in 0..MAX_PUBLICATION_RETRIES {
            if handle.instance_id != self.instance_id() {
                return Err(IndexError::ForeignInstance);
            }
            let view = self.view()?;
            let actual = view
                .file_version_including_inactive(handle.file_id)?
                .ok_or(IndexError::UnknownFile)?;
            if actual != handle.version {
                return Err(IndexError::FileConflict {
                    file: handle.file_id,
                    expected: handle.version,
                    actual,
                });
            }
            if view.file_entry(handle.file_id)?.is_none() {
                return Err(IndexError::UnknownFile);
            }
            let verified = verify_old_content(&view, handle, old_content).into_fallback()?;
            let (expected, next, retired_records, affected_collections) =
                self.stage_prepared_delete(handle)?;
            if expected != view.revision() {
                conflict = Some(IndexError::RevisionConflict {
                    expected: view.revision(),
                    actual: expected,
                });
                continue;
            }
            let (next, strategy, fallback_reason) = match verified {
                Ok(plan) if plan.requires_base_compaction() => {
                    match compact_verified_base(&next).into_fallback()? {
                        Ok(compacted) => (compacted, DeleteStrategy::VerifiedPhysicalRemoval, None),
                        Err(reason) => (next, DeleteStrategy::Tombstone, Some(reason)),
                    }
                }
                Ok(plan) => match remove_verified_old_shards(&next, &plan, handle.file_id)
                    .into_fallback()?
                {
                    Ok(compacted) => (compacted, DeleteStrategy::VerifiedPhysicalRemoval, None),
                    Err(reason) => (next, DeleteStrategy::Tombstone, Some(reason)),
                },
                Err(reason) => (next, DeleteStrategy::Tombstone, Some(reason)),
            };
            match self.publish_prepared(expected, next) {
                Ok(revision) => {
                    return Ok(VerifiedDeleteResult {
                        deletion: DeleteResult {
                            revision,
                            file: handle.file_id,
                            retired_records,
                            affected_collections,
                            strategy,
                        },
                        fallback_reason,
                    });
                }
                Err(error @ IndexError::RevisionConflict { .. }) => conflict = Some(error),
                Err(error) => return Err(error),
            }
        }
        Err(conflict.expect("publication retry always captures a conflict"))
    }
}

pub(crate) mod preparation;
mod staging;
#[cfg(test)]
mod tests;
mod verified;
