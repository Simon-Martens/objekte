use std::path::Path;
use std::time::SystemTime;

use super::relative_path;
use crate::construction::batch::SourceFileInput;
use crate::construction::file_preparation::{
    BytesPreparationError, FilePreparationBinding, PreparedCollectionBuild, PreparedFileBuild,
    prepare_file_build, prepare_file_build_from_bytes,
};
use crate::construction::segment_admission::SegmentAdmission;
use crate::index::segment::{IndexedCollectionSegment, SegmentId};
use crate::inventory::discovery::selects_file;
use crate::inventory::{FileEntry, FileStatus, SourceRevision};
use crate::lifecycle::{FileHandle, Index, IndexError, IndexRevision, IndexView};
use crate::model::FileId;
use crate::repository::Repository;

/// Private work captured before the mutation writer validates and publishes
/// one file across all eligible collections.
pub(crate) struct PreparedAdd {
    pub(crate) expected_revision: IndexRevision,
    pub(crate) configuration_identity: [u8; 32],
    pub(crate) file: PreparedFileBuild,
}

/// A complete replacement captured against one file incarnation. The writer
/// checks both the view revision and file token before replacing its slots.
pub(crate) struct PreparedReplacement {
    pub(crate) expected_revision: IndexRevision,
    pub(crate) expected_handle: FileHandle,
    pub(crate) configuration_identity: [u8; 32],
    pub(crate) file: PreparedFileBuild,
}

impl Index {
    /// Prepare a selected source without holding the publication or writer
    /// lock. The eventual commit revalidates the captured root and file ID.
    pub(crate) fn prepare_add_file(&self, path: &Path) -> Result<PreparedAdd, IndexError> {
        let view = self.view()?;
        let id = FileId(
            u32::try_from(view.0.catalog.allocated_files())
                .map_err(|_| IndexError::ResourceLimit("too many files".into()))?,
        );
        let file = prepare_file_at(&view, path, id)?;
        Ok(PreparedAdd {
            expected_revision: view.revision(),
            configuration_identity: view.configuration_identity(),
            file,
        })
    }

    /// Prepare an update or move after the application has changed the
    /// source file. The existing file ID survives; only its version advances.
    pub(crate) fn prepare_replace_file(
        &self,
        handle: FileHandle,
        path: &Path,
    ) -> Result<PreparedReplacement, IndexError> {
        if handle.instance_id != self.instance_id() {
            return Err(IndexError::ForeignInstance);
        }
        let view = self.view()?;
        let actual = view
            .0
            .file_versions
            .get(handle.file_id)
            .ok_or(IndexError::UnknownFile)?;
        if actual != handle.version {
            return Err(IndexError::FileConflict {
                file: handle.file_id,
                expected: handle.version,
                actual,
            });
        }
        if !view.0.catalog.is_active(handle.file_id) {
            return Err(IndexError::UnknownFile);
        }
        let file = prepare_file_at(&view, path, handle.file_id)?;
        Ok(PreparedReplacement {
            expected_revision: view.revision(),
            expected_handle: handle,
            configuration_identity: view.configuration_identity(),
            file,
        })
    }
}
pub(super) fn index_prepared_file(
    repository: &Repository,
    source: &SourceFileInput,
    collections: Vec<PreparedCollectionBuild>,
    first_segment_id: u64,
    policy: crate::index::segment::SegmentPolicy,
) -> Result<(FileEntry, Vec<IndexedCollectionSegment>), IndexError> {
    let options = repository.options();
    let mut indexed = Vec::with_capacity(collections.len());
    let mut next_segment_id = first_segment_id;
    for collection in collections {
        let state = repository
            .collections()
            .get(collection.collection.0 as usize)
            .ok_or_else(|| IndexError::InvalidPublication("collection missing".into()))?;
        if collection.collection != state.generation.collection
            || collection.source_id != state.required_file_reader().id()
            || collection.source_version != state.required_file_reader().version()
        {
            return Err(IndexError::Build(
                "prepared collection binding changed".into(),
            ));
        }
        let mut admission = SegmentAdmission::with_limits_and_id(
            collection.collection,
            &state.required_file_reader(),
            policy,
            SegmentId::new(next_segment_id),
        );
        next_segment_id = next_segment_id
            .checked_add(1)
            .ok_or_else(|| IndexError::ResourceLimit("segment identity overflow".into()))?;
        let mut finish = |admission: SegmentAdmission| -> Result<(), IndexError> {
            indexed.push(
                admission
                    .finish()
                    .into_indexed(
                        &state.definition,
                        options.bigram_alphabet,
                        options.inference,
                        repository.collections().len(),
                        &repository.workers,
                    )
                    .map_err(IndexError::Build)?,
            );
            Ok(())
        };
        for batch in collection.contributions {
            if batch.record_count() != 0
                && admission.file_count() != 0
                && !admission.can_admit_batch(&batch)
            {
                finish(std::mem::replace(
                    &mut admission,
                    SegmentAdmission::with_limits_and_id(
                        collection.collection,
                        &state.required_file_reader(),
                        policy,
                        SegmentId::new(next_segment_id),
                    ),
                ))?;
                next_segment_id = next_segment_id
                    .checked_add(1)
                    .ok_or_else(|| IndexError::ResourceLimit("segment identity overflow".into()))?;
            }
            admission.admit_batch(batch).map_err(IndexError::Build)?;
            if admission.at_capacity() {
                finish(std::mem::replace(
                    &mut admission,
                    SegmentAdmission::with_limits_and_id(
                        collection.collection,
                        &state.required_file_reader(),
                        policy,
                        SegmentId::new(next_segment_id),
                    ),
                ))?;
                next_segment_id = next_segment_id
                    .checked_add(1)
                    .ok_or_else(|| IndexError::ResourceLimit("segment identity overflow".into()))?;
            }
        }
        if admission.file_count() != 0 {
            finish(admission)?;
        }
    }
    let memberships = indexed.iter().map(|segment| segment.collection).collect();
    let mut entry = FileEntry {
        id: source.id,
        relative_path: source.relative_path.clone().into(),
        revision: source.revision,
        created_ns: source.created_ns,
        collections: memberships,
        status: FileStatus::Valid,
        diagnostics: Vec::new(),
        omitted_diagnostics: 0,
    };
    for segment in &indexed {
        let file = segment
            .file(source.id)
            .ok_or_else(|| IndexError::InvalidPublication("segment file entry missing".into()))?;
        entry.status = entry.status.max(file.entry.status);
        entry.omitted_diagnostics = entry
            .omitted_diagnostics
            .saturating_add(file.entry.omitted_diagnostics);
        for diagnostic in &file.entry.diagnostics {
            entry
                .record_diagnostic(diagnostic.clone())
                .map_err(IndexError::InvalidPublication)?;
        }
    }
    Ok((entry, indexed))
}

fn prepare_file_at(
    view: &IndexView,
    path: &Path,
    id: FileId,
) -> Result<PreparedFileBuild, IndexError> {
    let repository = view.repository();
    let root = &repository.files().required_root();
    let relative = if path.is_absolute() {
        path.strip_prefix(root)
            .map_err(|_| IndexError::SourceUnavailable("file is outside root".into()))?
    } else {
        path
    };
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return Err(IndexError::SourceUnavailable(
            "file path must be repository-relative and contained".into(),
        ));
    }
    if view
        .0
        .catalog
        .id_for(relative)
        .is_some_and(|other| other != id)
    {
        return Err(IndexError::DestinationExists);
    }
    let mut bindings = Vec::new();
    for collection in repository.collections() {
        if collection.file_source().is_none() {
            continue;
        }
        if selects_file(
            root,
            &collection.required_file_source().source_path,
            &collection.required_file_source().discovery,
            &collection.required_file_reader(),
            relative,
        )
        .map_err(IndexError::Build)?
        {
            bindings.push(FilePreparationBinding {
                collection: collection.generation.collection,
                definition: collection.required_file_definition(),
                source: &collection.required_file_reader(),
            });
        }
    }
    if bindings.is_empty() {
        return Err(IndexError::IncompatibleConfiguration(
            "no configured collection selects this file".into(),
        ));
    }
    let metadata = std::fs::metadata(root.join(relative))
        .map_err(|error| IndexError::Io(error.to_string()))?;
    let revision =
        crate::inventory::SourceRevision::from_metadata(&metadata).map_err(IndexError::Io)?;
    let created_ns = metadata
        .created()
        .ok()
        .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos() as i128);
    let source = SourceFileInput {
        id,
        relative_path: relative.to_owned().into(),
        revision,
        created_ns,
    };
    let options = repository.options();
    prepare_file_build(
        root,
        source,
        &bindings,
        options.bigram_alphabet,
        options.inference,
        1,
    )
    .map_err(IndexError::Build)
}

pub(super) fn created_ns(metadata: &std::fs::Metadata) -> Option<i128> {
    metadata
        .created()
        .ok()
        .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos() as i128)
}

pub(super) fn prepare_replacement_bytes(
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
    let root = &repository.files().required_root();
    let mut bindings = Vec::new();
    for collection in repository.collections() {
        if collection.file_source().is_none() {
            continue;
        }
        if selects_file(
            root,
            &collection.required_file_source().source_path,
            &collection.required_file_source().discovery,
            &collection.required_file_reader(),
            &relative,
        )
        .map_err(IndexError::Build)?
        {
            bindings.push(FilePreparationBinding {
                collection: collection.generation.collection,
                definition: collection.required_file_definition(),
                source: &collection.required_file_reader(),
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
        relative_path: relative.into(),
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
