use crate::index::storage::file_catalog::FileCatalogReader;
use std::path::Path;
use std::time::SystemTime;

use super::relative_path;
use crate::construction::batch::SourceFileInput;
use crate::construction::file_preparation::{
    FilePreparationBinding, PreparedCollectionBuild, PreparedFileBuild, prepare_observed_file_build,
};
use crate::index::segment::{IndexedCollectionSegment, SegmentId};
use crate::index::storage::segment::SegmentReader;
use crate::index::storage::segment::build::{FileSegmentBuild, SegmentBuilder, SegmentSeal};
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
            .catalog
            .version(handle.file_id)?
            .ok_or(IndexError::UnknownFile)?;
        if actual != handle.version {
            return Err(IndexError::FileConflict {
                file: handle.file_id,
                expected: handle.version,
                actual,
            });
        }
        if !view.0.catalog.is_active(handle.file_id)? {
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
pub(crate) fn index_prepared_file(
    repository: &Repository,
    source: &SourceFileInput,
    collections: Vec<PreparedCollectionBuild>,
    first_segment_id: u64,
    policy: crate::index::segment::SegmentPolicy,
) -> Result<(FileEntry, Vec<IndexedCollectionSegment>), IndexError> {
    let options = repository.options();
    let mut indexed = Vec::with_capacity(collections.len());
    let mut memberships = Vec::with_capacity(collections.len());
    let mut next_segment_id = first_segment_id;
    for collection in collections {
        if memberships.contains(&collection.collection) {
            return Err(IndexError::InvalidPublication(
                "repeated prepared file binding".into(),
            ));
        }
        memberships.push(collection.collection);
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
        if !collection.indexed.is_empty() {
            debug_assert!(collection.contributions.is_empty());
            for mut segment in collection.indexed {
                segment.id = SegmentId::new(next_segment_id);
                next_segment_id = next_segment_id
                    .checked_add(1)
                    .ok_or_else(|| IndexError::ResourceLimit("segment identity overflow".into()))?;
                indexed.push(segment);
            }
            continue;
        }
        let source = state.required_file_reader();
        let mut fresh = || -> Result<_, IndexError> {
            let id = SegmentId::new(next_segment_id);
            next_segment_id = next_segment_id
                .checked_add(1)
                .ok_or_else(|| IndexError::ResourceLimit("segment identity overflow".into()))?;
            let builder = repository
                .storage_factory
                .start_file_segment(FileSegmentBuild {
                    collection: collection.collection,
                    source: &source,
                    policy,
                })?;
            Ok((id, builder))
        };
        let mut draft = fresh()?;
        let mut finish =
            |(id, admission): (SegmentId, Box<dyn SegmentBuilder>)| -> Result<(), IndexError> {
                #[cfg(test)]
                super::tests::reach_file_preparation_checkpoint("whole-document-finalization");
                let built = admission.seal(SegmentSeal {
                    id,
                    definition: &state.definition,
                    options,
                    collection_count: repository.collections().len(),
                    pool: &repository.workers,
                    profile: None,
                    observe_inference: false,
                    declared_orders: repository.storage_factory.declared_order_store(),
                })?;
                indexed.push(built.index);
                Ok(())
            };
        for batch in collection.contributions {
            if !draft.1.can_admit(&batch)? {
                finish(std::mem::replace(&mut draft, fresh()?))?;
            }
            draft.1.admit(batch, None)?;
            if draft.1.at_capacity() {
                finish(std::mem::replace(&mut draft, fresh()?))?;
            }
        }
        if draft.1.file_count() != 0 {
            finish(draft)?;
        }
    }
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
            .file_metadata(source.id)?
            .ok_or_else(|| IndexError::InvalidPublication("segment file entry missing".into()))?;
        entry.status = entry.status.max(file.status);
        entry.omitted_diagnostics = entry
            .omitted_diagnostics
            .saturating_add(file.omitted_diagnostics);
        for diagnostic in &file.diagnostics {
            entry
                .record_diagnostic(diagnostic.clone())
                .map_err(IndexError::InvalidPublication)?;
        }
    }
    Ok((entry, indexed))
}

pub(crate) fn prepare_file_at(
    view: &IndexView,
    path: &Path,
    id: FileId,
) -> Result<PreparedFileBuild, IndexError> {
    let repository = view.repository();
    let root = repository
        .repository_root()
        .ok_or_else(|| IndexError::SourceUnavailable("file catalog is not registered".into()))?;
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
        .id_for_path(relative)?
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
                factory: &repository.storage_factory,
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
    let path = root.join(relative);
    let opened = std::fs::File::open(&path);
    let metadata = match &opened {
        Ok(file) => file.metadata(),
        Err(_) => std::fs::metadata(&path),
    }
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
    prepare_observed_file_build(
        root,
        source,
        &bindings,
        options.bigram_alphabet,
        options.inference,
        1,
        opened,
        None,
        Some(repository),
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
        .file_version_including_inactive(handle.file_id)?
        .ok_or(IndexError::UnknownFile)?;
    if actual != handle.version {
        return Err(IndexError::FileConflict {
            file: handle.file_id,
            expected: handle.version,
            actual,
        });
    }
    view.file_entry(handle.file_id)?
        .ok_or(IndexError::UnknownFile)?;
    let relative = relative_path(&view, path)?;
    if view
        .file_handle_for_path(&relative)?
        .is_some_and(|other| other.file_id != handle.file_id)
    {
        return Err(IndexError::DestinationExists);
    }
    let repository = view.repository();
    let root = repository.required_file_root();
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
                factory: &repository.storage_factory,
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
    // The byte form is a conditional update, never an alternative source of
    // indexed file content. Compare during the managed read below.
    let source_path = root.join(&relative);
    let opened = std::fs::File::open(&source_path)
        .map_err(|error| IndexError::SourceUnavailable(error.to_string()))?;
    let metadata = opened
        .metadata()
        .map_err(|error| IndexError::SourceUnavailable(error.to_string()))?;
    let observed =
        SourceRevision::from_metadata(&metadata).map_err(IndexError::SourceUnavailable)?;
    if observed != revision {
        return Err(IndexError::SourceUnavailable(
            "declared source revision differs from core observation".into(),
        ));
    }
    let input = SourceFileInput {
        id: handle.file_id,
        relative_path: relative.into(),
        revision: observed,
        created_ns: created_ns(&metadata),
    };
    let options = repository.options();
    let file = prepare_observed_file_build(
        root,
        input,
        &bindings,
        options.bigram_alphabet,
        options.inference,
        1,
        Ok(opened),
        Some(bytes),
        Some(repository),
    )
    .map_err(|error| {
        if error.contains("supplied bytes differ")
            || error.contains("supplied byte comparison")
            || error.contains("stale source")
        {
            IndexError::SourceUnavailable(error)
        } else {
            IndexError::Build(error)
        }
    })?;
    Ok(PreparedReplacement {
        expected_revision: view.revision(),
        expected_handle: handle,
        configuration_identity: view.configuration_identity(),
        file,
    })
}
