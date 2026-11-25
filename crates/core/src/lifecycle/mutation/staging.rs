use crate::index::storage::file_catalog::FileCatalogReader;
use std::sync::Arc;

use crate::construction::file_preparation::PreparedFileBuild;
use crate::index::segment::IndexedCollectionSegment;
use crate::inventory::FileEntry;
use crate::lifecycle::mutation::preparation::{
    PreparedAdd, PreparedReplacement, index_prepared_file,
};
use crate::lifecycle::{FileHandle, Index, IndexError, IndexRevision, IndexView, PublishedIndex};

pub(super) struct IndexedFilePlacement {
    pub(super) entry: FileEntry,
    segments: Vec<Arc<IndexedCollectionSegment>>,
}

impl IndexedFilePlacement {
    pub(super) fn prepare(
        view: &IndexView,
        file: &mut PreparedFileBuild,
    ) -> Result<Self, IndexError> {
        let (entry, segments) = index_prepared_file(
            view.repository(),
            &file.source,
            std::mem::take(&mut file.collections),
            0,
            view.repository().options().segment_policy,
        )?;
        Ok(Self {
            entry,
            segments: segments.into_iter().map(Arc::new).collect(),
        })
    }

    fn stage(&self, next: &mut PublishedIndex) -> Result<(), IndexError> {
        for segment in &self.segments {
            next.stage_reusable_file_segment(segment)?;
        }
        Ok(())
    }
}

impl Index {
    pub(crate) fn stage_prepared_delete(
        &self,
        handle: FileHandle,
    ) -> Result<(IndexRevision, PublishedIndex, u64, Vec<String>), IndexError> {
        if handle.instance_id != self.instance_id() {
            return Err(IndexError::ForeignInstance);
        }
        let current = self.view()?;
        let actual = current
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
        if !current.0.catalog.is_active(handle.file_id)? {
            return Err(IndexError::UnknownFile);
        }
        let file = current
            .0
            .catalog
            .file(handle.file_id)?
            .ok_or(IndexError::UnknownFile)?;
        let mut next = (*current.0).clone();
        let retired_records = next.retire_file_records(&file)?;
        next.refresh_record_links(handle.file_id, None)?;
        let affected_collections = file
            .collections
            .iter()
            .map(|id| {
                next.repository.collections()[id.0 as usize]
                    .definition
                    .name
                    .clone()
            })
            .collect();
        next.catalog = next.catalog.retired(handle.file_id)?;
        next.file_versions = next
            .file_versions
            .advanced(handle.file_id)
            .map_err(IndexError::ResourceLimit)?
            .0;
        // Pure deletions have no shard admission to trigger retirement work.
        for collection in &file.collections {
            next.make_room_for_segment(collection.0 as usize)?;
        }
        next.revision = current.revision().next()?;
        Ok((
            current.revision(),
            next,
            retired_records,
            affected_collections,
        ))
    }
}

impl Index {
    /// Build all affected collection segments and metadata privately. This
    /// returns an unpublished root; its captured revision must still be
    /// checked by the mutation writer before the eventual root swap.
    pub(crate) fn stage_prepared_add(
        &self,
        mut prepared: PreparedAdd,
    ) -> Result<PublishedIndex, IndexError> {
        let view = self.view()?;
        let file = IndexedFilePlacement::prepare(&view, &mut prepared.file)?;
        Self::stage_file_add(&view, &prepared, &file)
    }

    pub(super) fn stage_file_add(
        view: &IndexView,
        prepared: &PreparedAdd,
        file: &IndexedFilePlacement,
    ) -> Result<PublishedIndex, IndexError> {
        if view.revision() != prepared.expected_revision {
            return Err(IndexError::RevisionConflict {
                expected: prepared.expected_revision,
                actual: view.revision(),
            });
        }
        if view.configuration_identity() != prepared.configuration_identity {
            return Err(IndexError::IncompatibleConfiguration(
                "configuration changed during file preparation".into(),
            ));
        }
        if view.0.catalog.allocated_files() != prepared.file.source.id.0 as usize {
            return Err(IndexError::InvalidPublication(
                "prepared file ID is no longer next".into(),
            ));
        }
        if view
            .0
            .catalog
            .id_for_path(&prepared.file.source.relative_path)?
            .is_some()
        {
            return Err(IndexError::DestinationExists);
        }
        let mut next = (*view.0).clone();
        next.catalog = next.catalog.appended(
            file.entry.clone(),
            prepared.file.fingerprint,
            Some(prepared.configuration_identity),
        )?;
        next.file_versions = next
            .file_versions
            .appended()
            .map_err(IndexError::ResourceLimit)?;
        file.stage(&mut next)?;
        next.revision = view.revision().next()?;
        Ok(next)
    }
    /// Stage an update or move with the old incarnation retired in every
    /// collection. The returned root is not visible until the writer checks
    /// the captured revision and publishes it.
    pub(crate) fn stage_prepared_replace(
        &self,
        mut prepared: PreparedReplacement,
    ) -> Result<PublishedIndex, IndexError> {
        let view = self.view()?;
        let file = IndexedFilePlacement::prepare(&view, &mut prepared.file)?;
        self.stage_file_replace(&view, &prepared, &file)
    }

    pub(super) fn stage_file_replace(
        &self,
        view: &IndexView,
        prepared: &PreparedReplacement,
        file: &IndexedFilePlacement,
    ) -> Result<PublishedIndex, IndexError> {
        if prepared.expected_handle.instance_id != self.instance_id() {
            return Err(IndexError::ForeignInstance);
        }
        if view.revision() != prepared.expected_revision {
            return Err(IndexError::RevisionConflict {
                expected: prepared.expected_revision,
                actual: view.revision(),
            });
        }
        if view.configuration_identity() != prepared.configuration_identity {
            return Err(IndexError::IncompatibleConfiguration(
                "configuration changed during file preparation".into(),
            ));
        }
        let file_id = prepared.expected_handle.file_id;
        if prepared.file.source.id != file_id {
            return Err(IndexError::InvalidPublication(
                "prepared replacement file ID changed".into(),
            ));
        }
        let actual = view
            .0
            .file_versions
            .get(file_id)
            .ok_or(IndexError::UnknownFile)?;
        if actual != prepared.expected_handle.version {
            return Err(IndexError::FileConflict {
                file: file_id,
                expected: prepared.expected_handle.version,
                actual,
            });
        }
        let previous = view
            .0
            .catalog
            .file(file_id)?
            .ok_or(IndexError::UnknownFile)?;
        if view
            .0
            .catalog
            .id_for_path(&prepared.file.source.relative_path)?
            .is_some_and(|other| other != file_id)
        {
            return Err(IndexError::DestinationExists);
        }
        let mut entry = file.entry.clone();
        let mut next = (*view.0).clone();
        next.retire_file_records(&previous)?;
        for collection in next.refresh_record_links(file_id, Some(&entry))? {
            if !entry.collections.contains(&collection) {
                entry.collections.push(collection);
            }
        }
        next.catalog = next.catalog.replaced(
            entry,
            prepared.file.fingerprint,
            Some(prepared.configuration_identity),
        )?;
        next.file_versions = next
            .file_versions
            .advanced(file_id)
            .map_err(IndexError::ResourceLimit)?
            .0;
        file.stage(&mut next)?;
        // A replacement may remove a collection contribution without adding
        // a new shard there. Recheck its retired ratio after the catalog swap.
        for collection in &previous.collections {
            next.make_room_for_segment(collection.0 as usize)?;
        }
        next.revision = view.revision().next()?;
        Ok(next)
    }
}
