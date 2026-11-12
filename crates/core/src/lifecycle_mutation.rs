//! Public file mutations backed by the lifecycle preparation and publication
//! path. Source files remain owned by the application.

use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use sha2::{Digest, Sha256};

use crate::build::{
    BytesPreparationError, FilePreparationBinding, PreparedFileBuild, SourceFileInput,
    prepare_file_build_from_bytes,
};
use crate::discovery::selects_file;
use crate::inventory::SourceRevision;
use crate::lifecycle::{
    DeleteResult, DeleteStrategy, FileHandle, Index, IndexError, IndexRevision, IndexView,
    PreparedReplacement, PublishedIndex,
};
use crate::lifecycle_compaction::compact_published;
use crate::lifecycle_verified::{
    preserve_partition_record_slots, preserve_single_record_slots, remove_verified_old_shards,
    verify_old_content,
};
use crate::model::{CollectionId, MetadataField, PathRoot};
use crate::repository::IdentityPolicy;

const MAX_PUBLICATION_RETRIES: usize = 8;

fn compact_verified_base(next: &PublishedIndex) -> Result<PublishedIndex, VerifiedFallbackReason> {
    compact_published(next, next.revision).map_err(|error| match error {
        IndexError::ResourceLimit(_) => VerifiedFallbackReason::CompactionBudget,
        _ => VerifiedFallbackReason::CompactionUnavailable,
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
    let root = &repository.files().root;
    for collection in repository.collections() {
        let selected = selects_file(
            root,
            &collection.definition.source_path,
            &collection.definition.discovery,
            &collection.source,
            destination,
        )
        .map_err(IndexError::Build)?;
        if selected != old.collections.contains(&collection.generation.collection) {
            return Ok(false);
        }
        if selected
            && (collection.source.is_extension()
                || !matches!(
                    collection.definition.identity,
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

fn created_ns(metadata: &std::fs::Metadata) -> Option<i128> {
    metadata
        .created()
        .ok()
        .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos() as i128)
}

fn hash_destination(path: &Path) -> Result<([u8; 32], SourceRevision, Option<i128>), IndexError> {
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
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = source
            .read(&mut buffer)
            .map_err(|error| IndexError::Io(error.to_string()))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
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
    Ok((hasher.finalize().into(), revision, creation))
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
                        fallback_reason: None,
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
        self.replace_file(handle, FileMutationKind::Replaced, options, |index| {
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
        if let Some(moved) = self.try_content_preserving_move(handle, destination)? {
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
    ) -> Result<Option<FileMutationResult>, IndexError> {
        if handle.instance_id != self.instance_id() {
            return Err(IndexError::ForeignInstance);
        }
        let mut conflict = None;
        for _ in 0..MAX_PUBLICATION_RETRIES {
            let view = self.view()?;
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
            let old = view
                .file_entry(handle.file_id)
                .ok_or(IndexError::UnknownFile)?;
            let relative = relative_path(&view, destination)?;
            if relative.as_path() == old.relative_path.as_path() {
                return Ok(None);
            }
            if view
                .file_handle_for_path(&relative)
                .is_some_and(|other| other.file_id != handle.file_id)
            {
                return Err(IndexError::DestinationExists);
            }
            if !path_independent_memberships(&view, old, &relative)? {
                return Ok(None);
            }
            let Some(fingerprint) = view.file_fingerprint(handle.file_id) else {
                return Ok(None);
            };
            if view.file_configuration(handle.file_id) != Some(view.configuration_identity()) {
                return Ok(None);
            }
            let absolute = view.repository().files().root.join(&relative);
            let (digest, revision, creation) = hash_destination(&absolute)?;
            if digest != fingerprint || revision != old.revision || creation != old.created_ns {
                return Ok(None);
            }
            let mut next = (*view.0).clone();
            let mut relocated = old.clone();
            relocated.relative_path = relative.into();
            next.catalog = next
                .catalog
                .replaced(
                    relocated,
                    Some(fingerprint),
                    Some(view.configuration_identity()),
                )
                .map_err(IndexError::InvalidPublication)?;
            next.file_versions = next
                .file_versions
                .advanced(handle.file_id)
                .map_err(IndexError::ResourceLimit)?
                .0;
            next.revision = view.revision().next()?;
            let file = FileHandle {
                instance_id: self.instance_id(),
                file_id: handle.file_id,
                version: next.file_versions.get(handle.file_id).ok_or_else(|| {
                    IndexError::InvalidPublication("moved file version missing".into())
                })?,
            };
            let indexed_records = view
                .file_record_count(handle.file_id)
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
            let unchanged = previous.relative_path.as_path()
                == prepared.file.source.relative_path.as_path()
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
                    fallback_reason: None,
                });
            }
            let verified = match options.strategy {
                UpdateStrategy::AppendReplacement => None,
                UpdateStrategy::PreferVerified => Some(match options.old_content {
                    Some(bytes) => verify_old_content(&view, handle, bytes),
                    None => Err(VerifiedFallbackReason::MissingOldContent),
                }),
            };
            let expected = prepared.expected_revision;
            let next = match self.stage_prepared_replace(prepared) {
                Ok(next) => next,
                Err(error @ IndexError::RevisionConflict { .. }) => {
                    conflict = Some(error);
                    continue;
                }
                Err(error) => return Err(error),
            };
            let (next, strategy, fallback_reason) = match verified {
                Some(Ok(plan)) => {
                    if plan.requires_base_compaction() {
                        match compact_verified_base(&next) {
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
                        preserve_single_record_slots(&view, &next, &plan, handle.file_id).or_else(
                            || preserve_partition_record_slots(&view, &next, &plan, handle.file_id),
                        )
                    {
                        (
                            preserved,
                            FileMutationStrategy::VerifiedSlotPreserving,
                            None,
                        )
                    } else {
                        match remove_verified_old_shards(&next, &plan, handle.file_id) {
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
                .file_version_including_inactive(handle.file_id)
                .ok_or(IndexError::UnknownFile)?;
            if actual != handle.version {
                return Err(IndexError::FileConflict {
                    file: handle.file_id,
                    expected: handle.version,
                    actual,
                });
            }
            if view.file_entry(handle.file_id).is_none() {
                return Err(IndexError::UnknownFile);
            }
            let verified = verify_old_content(&view, handle, old_content);
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
                Ok(plan) if plan.requires_base_compaction() => match compact_verified_base(&next) {
                    Ok(compacted) => (compacted, DeleteStrategy::VerifiedPhysicalRemoval, None),
                    Err(reason) => (next, DeleteStrategy::Tombstone, Some(reason)),
                },
                Ok(plan) => match remove_verified_old_shards(&next, &plan, handle.file_id) {
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


#[cfg(test)]
#[path = "lifecycle_mutation.test.rs"]
mod tests;
