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
mod worker;

pub(crate) struct MemoryStorageFactory;

impl super::storage::StorageFactory for MemoryStorageFactory {
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
