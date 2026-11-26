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

impl super::traits::StorageFactory for MemoryStorageFactory {
    fn import_file_segment(
        &self,
        source: &dyn super::traits::segment::SegmentReader,
        layout: super::traits::segment::transfer::FileSegmentLayout<'_>,
        cancelled: &mut dyn FnMut() -> bool,
    ) -> super::traits::transfer::StorageResult<Option<super::traits::segment::SegmentHandle>> {
        segment::import::import(source, layout, cancelled)
    }

    fn project_segment_partitions(
        &self,
        source: &dyn super::traits::segment::SegmentReader,
        partitions: &[super::traits::segment::maintenance::SegmentPartition],
        catalog: &dyn super::traits::file_catalog::FileCatalogReader,
        cancelled: &mut dyn FnMut() -> bool,
    ) -> super::traits::transfer::StorageResult<Vec<super::traits::segment::SegmentHandle>> {
        let editor = source.maintenance();
        if let Some(native) = editor.and_then(|editor| {
            (editor as &dyn std::any::Any).downcast_ref::<segment::IndexedCollectionSegment>()
        }) {
            use super::traits::segment::maintenance::SegmentMaintenance;
            return SegmentMaintenance::project_partitions(native, partitions, catalog, cancelled);
        }
        if let [partition] = partitions {
            use super::traits::segment::transfer::FileSegmentLayout;

            return self
                .import_file_segment(
                    source,
                    FileSegmentLayout::Dense { partition, catalog },
                    cancelled,
                )?
                .map(|part| vec![part])
                .ok_or(super::traits::transfer::StorageError::Unsupported {
                    operation: "import segment into memory partitions",
                });
        }
        segment::import::import_partitions(source, partitions, catalog, cancelled)
    }

    fn consolidate_segments(
        &self,
        segments: &[&dyn super::traits::segment::SegmentReader],
        max_records: usize,
    ) -> super::traits::transfer::StorageResult<
        Option<super::traits::segment::maintenance::ConsolidatedSegment>,
    > {
        segment::maintenance::consolidate(segments, max_records)
    }

    #[cfg(test)]
    fn base_segment(
        &self,
        collection: &crate::repository::state::CollectionState,
        files: &dyn super::traits::file_catalog::FileCatalogReader,
    ) -> super::traits::transfer::StorageResult<super::traits::segment::SegmentHandle> {
        segment::maintenance::base_segment(collection, files)
    }

    fn start_file_segment(
        &self,
        input: super::traits::segment::build::FileSegmentBuild<'_>,
    ) -> super::traits::transfer::StorageResult<
        Box<dyn super::traits::segment::build::SegmentBuilder>,
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
        input: super::traits::segment::build::DirectSegmentBuild<'_>,
        seal: super::traits::segment::build::SegmentSeal<'_>,
    ) -> super::traits::transfer::StorageResult<super::traits::segment::build::BuiltSegment> {
        use super::traits::segment::build::DirectSegmentBodies;
        use super::traits::transfer::StorageError;
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
    ) -> super::traits::transfer::StorageResult<super::traits::segment::RetainedBlock> {
        Ok(std::sync::Arc::new(segment::MemoryRetainedBlock(bodies)))
    }

    fn inferred_order(
        &self,
        rows: super::traits::order::InferredOrderRows,
    ) -> super::traits::transfer::StorageResult<
        std::sync::Arc<dyn super::traits::order::OrderColumnReader>,
    > {
        Ok(std::sync::Arc::new(sorting::SortColumn::from(rows)))
    }

    fn live_records(
        &self,
        slots: usize,
        initial: super::traits::liveness::InitialLiveness<'_>,
    ) -> super::traits::transfer::StorageResult<super::traits::liveness::ActiveRecords> {
        use super::traits::liveness::{ActiveRecords, InitialLiveness};
        match initial {
            InitialLiveness::All => inventory::MemoryLiveSlots::all(slots),
            InitialLiveness::Selected(members) => {
                inventory::MemoryLiveSlots::from_membership(slots, members)
            }
        }
        .map(ActiveRecords::new)
        .map_err(super::traits::transfer::StorageError::ResourceLimit)
    }

    fn prepare_file_records(
        &self,
        batch: super::traits::worker::FileWorkerBatch<'_>,
    ) -> super::traits::transfer::StorageResult<crate::construction::batch::BatchContribution> {
        prepare_file_with_components(batch, self)
    }

    fn prepare_direct_records(
        &self,
        batch: super::traits::worker::DirectRecordBatch<'_>,
    ) -> super::traits::transfer::StorageResult<super::traits::worker::PreparedDirectBatch> {
        prepare_direct_with_components(batch, self)
    }

    fn start_file_discovery(
        &self,
        root: std::path::PathBuf,
    ) -> super::traits::transfer::StorageResult<
        Box<dyn super::traits::file_catalog::FileDiscoveryBuilder>,
    > {
        Ok(Box::new(inventory::FileInventory::new(root)))
    }

    fn file_catalog(
        &self,
        base: &dyn super::traits::file_catalog::FileCatalogBase,
        version: crate::inventory::FileVersion,
    ) -> super::traits::transfer::StorageResult<super::traits::file_catalog::FileCatalogHandle>
    {
        Ok(std::sync::Arc::new(file_catalog::FileCatalog::from_base(
            base, version,
        )?))
    }

    fn restore_file_catalog(
        &self,
        base: &dyn super::traits::file_catalog::FileCatalogBase,
        source: &dyn super::traits::file_catalog::FileCatalogRevision,
        cancelled: &dyn Fn() -> bool,
    ) -> super::traits::transfer::StorageResult<super::traits::file_catalog::FileCatalogHandle>
    {
        Ok(std::sync::Arc::new(
            file_catalog::FileCatalog::restore_revision(base, source, cancelled)?,
        ))
    }

    fn segment_catalog(
        &self,
    ) -> super::traits::transfer::StorageResult<
        Box<dyn super::traits::segment_catalog::SegmentCatalogEditor>,
    > {
        Ok(Box::new(segment_catalog::RecordRoutes::empty()))
    }
}

pub(crate) fn prepare_file_with_components(
    batch: super::traits::worker::FileWorkerBatch<'_>,
    factory: &dyn super::traits::StorageFactory,
) -> super::traits::transfer::StorageResult<crate::construction::batch::BatchContribution> {
    let worker = worker::MemoryWorker::new(
        batch.definition,
        batch.alphabet,
        batch.inference,
        batch.inference_jobs,
        factory.posting_store(),
        factory.gram_store(),
        factory.relation_store(),
    )
    .map_err(|error| super::traits::transfer::StorageError::invalid("start file worker", error))?;
    crate::construction::batch::prepare_file_records(batch, worker)
}

pub(crate) fn prepare_direct_with_components(
    batch: super::traits::worker::DirectRecordBatch<'_>,
    factory: &dyn super::traits::StorageFactory,
) -> super::traits::transfer::StorageResult<super::traits::worker::PreparedDirectBatch> {
    let worker = worker::MemoryWorker::new(
        batch.declaration,
        batch.options.bigram_alphabet,
        batch.options.inference,
        1,
        factory.posting_store(),
        factory.gram_store(),
        factory.relation_store(),
    )
    .map_err(|error| {
        super::traits::transfer::StorageError::invalid("start direct worker", error)
    })?;
    crate::construction::batch::prepare_direct_records(batch, worker)
}
