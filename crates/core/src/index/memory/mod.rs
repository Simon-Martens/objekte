//! Native in-memory index storage.

pub(crate) mod access;
pub mod bigram;
pub(crate) mod file_catalog;
mod file_scalar;
pub(crate) mod inference;
pub(crate) mod inventory;
pub mod posting;
pub mod relation;
pub mod segment;
pub(crate) mod segment_catalog;
pub mod sorting;
mod transfer;
mod worker;

pub(crate) struct MemoryStorageFactory;

impl super::storage::StorageFactory for MemoryStorageFactory {
    fn import_file_segment(
        &self,
        source: &dyn super::storage::segment::SegmentReader,
        layout: super::storage::segment::transfer::FileSegmentLayout<'_>,
        cancelled: &mut dyn FnMut() -> bool,
    ) -> super::storage::transfer::StorageResult<Option<super::storage::segment::SegmentHandle>>
    {
        segment::import::import(source, layout, cancelled)
    }

    fn project_segment_partitions(
        &self,
        source: &dyn super::storage::segment::SegmentReader,
        partitions: &[super::storage::segment::maintenance::SegmentPartition],
        catalog: &dyn super::storage::file_catalog::FileCatalogReader,
        cancelled: &mut dyn FnMut() -> bool,
    ) -> super::storage::transfer::StorageResult<Vec<super::storage::segment::SegmentHandle>> {
        let editor = source.maintenance();
        if let Some(native) = editor.and_then(|editor| {
            (editor as &dyn std::any::Any).downcast_ref::<segment::IndexedCollectionSegment>()
        }) {
            use super::storage::segment::maintenance::SegmentMaintenance;
            return SegmentMaintenance::project_partitions(native, partitions, catalog, cancelled);
        }
        segment::import::import_partitions(source, partitions, catalog, cancelled)
    }

    fn consolidate_segments(
        &self,
        segments: &[&dyn super::storage::segment::SegmentReader],
        max_records: usize,
    ) -> super::storage::transfer::StorageResult<
        Option<super::storage::segment::maintenance::ConsolidatedSegment>,
    > {
        segment::maintenance::consolidate(segments, max_records)
    }

    #[cfg(test)]
    fn base_segment(
        &self,
        collection: &crate::repository::state::CollectionState,
        files: &dyn super::storage::file_catalog::FileCatalogReader,
    ) -> super::storage::transfer::StorageResult<super::storage::segment::SegmentHandle> {
        segment::maintenance::base_segment(collection, files)
    }

    fn start_file_segment(
        &self,
        input: super::storage::segment::build::FileSegmentBuild<'_>,
    ) -> super::storage::transfer::StorageResult<
        Box<dyn super::storage::segment::build::SegmentBuilder>,
    > {
        Ok(Box::new(
            segment::admission::SegmentAdmission::with_limits_and_id(
                input.collection,
                input.source,
                input.policy,
                segment::SegmentId::new(0),
            ),
        ))
    }

    fn seal_direct_segment(
        &self,
        input: super::storage::segment::build::DirectSegmentBuild<'_>,
        seal: super::storage::segment::build::SegmentSeal<'_>,
    ) -> super::storage::transfer::StorageResult<super::storage::segment::build::BuiltSegment> {
        use super::storage::segment::build::DirectSegmentBodies;
        use super::storage::transfer::StorageError;
        let draft = match input.bodies {
            DirectSegmentBodies::Versioned(versions) => {
                segment::admission::CollectionSegment::from_provider(
                    seal.id,
                    input.collection,
                    input.source_id.into(),
                    input.source_version,
                    input.registration,
                    input.raw_covers_logical_fields,
                    input.headers,
                    versions.to_vec(),
                    input.contribution,
                )
            }
            DirectSegmentBodies::Retained(bodies) => {
                segment::admission::CollectionSegment::from_retained(
                    seal.id,
                    input.collection,
                    input.source_id.into(),
                    input.source_version,
                    input.registration,
                    input.raw_covers_logical_fields,
                    input.headers,
                    bodies,
                    input.contribution,
                )
            }
        }
        .map_err(|error| StorageError::invalid("admit direct segment", error))?;
        draft.seal(seal)
    }

    fn retain_record_bodies(
        &self,
        bodies: Vec<crate::records::RetainedBody>,
    ) -> super::storage::transfer::StorageResult<super::storage::segment::RetainedBlock> {
        Ok(std::sync::Arc::new(segment::MemoryRetainedBlock(bodies)))
    }

    fn inferred_order(
        &self,
        rows: super::storage::order::InferredOrderRows,
    ) -> super::storage::transfer::StorageResult<
        std::sync::Arc<dyn super::storage::order::OrderColumnReader>,
    > {
        Ok(std::sync::Arc::new(sorting::SortColumn::from(rows)))
    }

    fn live_records(
        &self,
        slots: usize,
        initial: super::storage::liveness::InitialLiveness<'_>,
    ) -> super::storage::transfer::StorageResult<super::storage::liveness::ActiveRecords> {
        use super::storage::liveness::{ActiveRecords, InitialLiveness};
        match initial {
            InitialLiveness::All => inventory::MemoryLiveSlots::all(slots),
            InitialLiveness::Selected(members) => {
                inventory::MemoryLiveSlots::from_membership(slots, members)
            }
        }
        .map(ActiveRecords::new)
        .map_err(super::storage::transfer::StorageError::ResourceLimit)
    }

    fn prepare_file_records(
        &self,
        batch: super::storage::worker::FileWorkerBatch<'_>,
    ) -> super::storage::transfer::StorageResult<crate::construction::batch::BatchContribution>
    {
        let worker = worker::MemoryWorker::new(
            batch.definition,
            batch.alphabet,
            batch.inference,
            batch.inference_jobs,
        )
        .map_err(|error| {
            super::storage::transfer::StorageError::invalid("start file worker", error)
        })?;
        crate::construction::batch::prepare_file_records(batch, worker)
    }

    fn prepare_direct_records(
        &self,
        batch: super::storage::worker::DirectRecordBatch<'_>,
    ) -> super::storage::transfer::StorageResult<super::storage::worker::PreparedDirectBatch> {
        let worker = worker::MemoryWorker::new(
            batch.declaration,
            batch.options.bigram_alphabet,
            batch.options.inference,
            1,
        )
        .map_err(|error| {
            super::storage::transfer::StorageError::invalid("start worker indexes", error)
        })?;
        crate::construction::batch::prepare_direct_records(batch, worker)
    }

    fn start_file_discovery(
        &self,
        root: std::path::PathBuf,
    ) -> super::storage::transfer::StorageResult<
        Box<dyn super::storage::file_catalog::FileDiscoveryBuilder>,
    > {
        Ok(Box::new(inventory::FileInventory::new(root)))
    }

    fn file_catalog(
        &self,
        base: &dyn super::storage::file_catalog::FileCatalogBase,
        version: crate::inventory::FileVersion,
    ) -> super::storage::transfer::StorageResult<super::storage::file_catalog::FileCatalogHandle>
    {
        Ok(std::sync::Arc::new(file_catalog::FileCatalog::from_base(
            base, version,
        )?))
    }

    fn restore_file_catalog(
        &self,
        base: &dyn super::storage::file_catalog::FileCatalogBase,
        source: &dyn super::storage::file_catalog::FileCatalogRevision,
        cancelled: &dyn Fn() -> bool,
    ) -> super::storage::transfer::StorageResult<super::storage::file_catalog::FileCatalogHandle>
    {
        Ok(std::sync::Arc::new(
            file_catalog::FileCatalog::restore_revision(base, source, cancelled)?,
        ))
    }

    fn segment_catalog(
        &self,
    ) -> super::storage::transfer::StorageResult<
        Box<dyn super::storage::segment_catalog::SegmentCatalogEditor>,
    > {
        Ok(Box::new(segment_catalog::RecordRoutes::empty()))
    }
}
