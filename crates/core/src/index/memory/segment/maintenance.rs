//! Native implementations of the sealed segment edit contract.

use std::any::Any;
use std::sync::Arc;

use super::IndexedCollectionSegment;
use crate::index::storage::file_catalog::FileCatalogReader;
use crate::index::storage::segment::maintenance::{
    ConsolidatedSegment, ProjectedSegment, RelinkedFile, SegmentMaintenance, SegmentPartition,
};
use crate::index::storage::segment::{SegmentHandle, SegmentReader};
use crate::index::storage::transfer::{StorageError, StorageResult};
use crate::inventory::{ActiveRecords, FileEntry};
use crate::model::{FileId, RecordSlot};
use crate::repository::state::GlobalInferenceSummary;

pub(in crate::index::memory) fn native_segment(
    reader: &dyn SegmentReader,
) -> Option<&IndexedCollectionSegment> {
    (reader as &dyn Any).downcast_ref()
}

pub(crate) fn consolidate(
    segments: &[&dyn SegmentReader],
    max_records: usize,
) -> StorageResult<Option<ConsolidatedSegment>> {
    let Some(native) = segments
        .iter()
        .map(|segment| native_segment(*segment))
        .collect::<Option<Vec<_>>>()
    else {
        return Ok(None);
    };
    let merged = IndexedCollectionSegment::consolidate(&native, max_records)
        .map_err(|error| StorageError::invalid("consolidate segments", error))?;
    Ok(Some(ConsolidatedSegment {
        payload: Arc::new(merged.index),
        offsets: merged.offsets,
    }))
}

impl SegmentMaintenance for IndexedCollectionSegment {
    fn project_active(
        &self,
        active: &ActiveRecords,
        max_slots: usize,
        retain_file: &mut dyn FnMut(&FileEntry) -> bool,
    ) -> StorageResult<ProjectedSegment> {
        let projected = self
            .compact_active(active, max_slots, retain_file)
            .map_err(|error| StorageError::invalid("project active segment", error))?;
        Ok(ProjectedSegment {
            payload: Arc::new(projected.index),
            old_to_new: projected.old_to_new,
        })
    }

    fn project_partitions(
        &self,
        partitions: &[SegmentPartition],
        catalog: &dyn FileCatalogReader,
        cancelled: &mut dyn FnMut() -> bool,
    ) -> StorageResult<Vec<SegmentHandle>> {
        Ok(
            IndexedCollectionSegment::project_partitions(self, partitions, catalog, cancelled)?
                .into_iter()
                .map(|part| Arc::new(part) as SegmentHandle)
                .collect(),
        )
    }

    fn remap_dense(&self, new_slots: &[RecordSlot]) -> StorageResult<SegmentHandle> {
        self.remap_dense(new_slots)
            .map(|part| Arc::new(part) as SegmentHandle)
            .map_err(|error| StorageError::invalid("remap dense segment", error))
    }

    fn reconcile_final_metadata(
        &self,
        catalog: &dyn FileCatalogReader,
        decision: Option<&GlobalInferenceSummary>,
    ) -> StorageResult<Option<SegmentHandle>> {
        Ok(self
            .reconcile_final_metadata(catalog, decision)?
            .map(|part| Arc::new(part) as SegmentHandle))
    }

    fn relink_file(
        &self,
        file: FileId,
        replacement: Option<&FileEntry>,
        root: Option<&std::path::Path>,
        collection_name: &str,
    ) -> StorageResult<Option<RelinkedFile>> {
        Ok(self
            .relink_file(file, replacement, root, collection_name)?
            .map(|edit| RelinkedFile {
                linked: edit.linked,
                updated: edit.updated.map(|part| Arc::new(part) as SegmentHandle),
            }))
    }
}

pub(crate) fn base_segment(
    collection: &crate::repository::state::CollectionState,
    files: &dyn crate::index::storage::file_catalog::FileCatalogReader,
) -> StorageResult<SegmentHandle> {
    use super::{SegmentBacking, SegmentFile};
    use std::collections::HashMap;
    let invalid = |detail| StorageError::invalid("project collection base", detail);
    let mut segment_files = Vec::new();
    let mut file_index = HashMap::new();
    for id in &collection.generation.files {
        let entry = files
            .file(*id)?
            .ok_or_else(|| invalid("base file missing".into()))?
            .into_owned();
        let records = collection
            .generation
            .file_records
            .records_for(*id)
            .ok_or_else(|| invalid("base file ownership missing".into()))?
            .to_vec();
        file_index.insert(*id, segment_files.len());
        segment_files.push(SegmentFile { entry, records });
    }
    let relations = collection
        .relations
        .iter()
        .map(|relation| {
            crate::index::relation::RelationContribution::from_resolved_index(
                relation,
                collection.generation.records.len(),
            )
            .map_err(invalid)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let records = collection.generation.records.clone();
    let backing = SegmentBacking::file_ranges(&records);
    let mut segment = IndexedCollectionSegment {
        id: crate::index::segment::SegmentId::new(1),
        collection: collection.generation.collection,
        source_id: collection.required_file_reader().id().into(),
        source_version: collection.required_file_reader().version(),
        files: segment_files,
        file_index,
        records: records
            .iter()
            .map(crate::records::RecordHeader::from)
            .collect(),
        backing,
        write_versions: Default::default(),
        raw_covers_logical_fields: true,
        id_lookup: collection.id_lookup.clone(),
        identity_order: collection.identity_order.clone(),
        postings: collection.postings.clone().into(),
        inference: collection.inference.clone(),
        custom_postings: collection.custom_postings.clone(),
        raw: collection.raw.clone().into(),
        fallback_text: collection.fallback_text.clone().into(),
        custom_text: collection.custom_text.clone(),
        logical_records: collection.logical_records.clone(),
        empty_array_records: collection.empty_array_records.clone(),
        sorting: collection.sorting.clone().into(),
        relations,
        record_parses: 0,
        failed_parses: 0,
        accounting: Default::default(),
    };
    segment.recompute_accounting();
    Ok(Arc::new(segment))
}
