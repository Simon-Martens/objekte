use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::*;
use crate::index::storage::StorageFactory;
use crate::index::storage::transfer::StorageError;
use crate::lifecycle::Index;
use crate::model::Value;
use crate::query::options::ReturnMode;
use crate::schema::Schema;

#[derive(Default)]
struct TableFactory {
    installs: AtomicUsize,
    worker_calls: AtomicUsize,
    file_worker_calls: AtomicUsize,
    fail_file_worker: AtomicBool,
    fail_seal: AtomicBool,
    fail_lookup: AtomicBool,
}

impl StorageFactory for Arc<TableFactory> {
    fn prepare_file_records(
        &self,
        batch: crate::index::storage::worker::FileWorkerBatch<'_>,
    ) -> StorageResult<crate::construction::batch::BatchContribution> {
        self.file_worker_calls.fetch_add(1, Ordering::Relaxed);
        let contribution = crate::index::storage::default_factory().prepare_file_records(batch)?;
        if self.fail_file_worker.swap(false, Ordering::Relaxed) {
            return Err(StorageError::invalid(
                "test worker",
                "injected prepared file failure",
            ));
        }
        Ok(contribution)
    }

    fn prepare_direct_records(
        &self,
        batch: crate::index::storage::worker::DirectRecordBatch<'_>,
    ) -> StorageResult<crate::index::storage::worker::PreparedDirectBatch> {
        self.worker_calls.fetch_add(1, Ordering::Relaxed);
        crate::index::storage::default_factory().prepare_direct_records(batch)
    }

    fn file_catalog(
        &self,
        base: &dyn crate::index::storage::file_catalog::FileCatalogBase,
        version: crate::inventory::FileVersion,
    ) -> StorageResult<crate::index::storage::file_catalog::FileCatalogHandle> {
        crate::index::storage::default_factory().file_catalog(base, version)
    }

    fn restore_file_catalog(
        &self,
        base: &dyn crate::index::storage::file_catalog::FileCatalogBase,
        source: &dyn crate::index::storage::file_catalog::FileCatalogRevision,
        cancelled: &dyn Fn() -> bool,
    ) -> StorageResult<crate::index::storage::file_catalog::FileCatalogHandle> {
        crate::index::storage::default_factory().restore_file_catalog(base, source, cancelled)
    }

    fn segment_catalog(&self) -> StorageResult<Box<dyn SegmentCatalogEditor>> {
        Ok(Box::new(TableDirectory {
            records: BTreeMap::new(),
            files: BTreeMap::new(),
            positions: BTreeMap::new(),
            control: Arc::clone(self),
        }))
    }
}

/// An independent route directory that uses ordered rows and stores no
/// in-memory backend route shards, index structures, or slot bitmaps.
#[derive(Clone)]
struct TableDirectory {
    records: BTreeMap<RecordId, (SegmentId, RecordSlot)>,
    files: BTreeMap<u32, Vec<SegmentId>>,
    positions: BTreeMap<SegmentId, usize>,
    control: Arc<TableFactory>,
}

impl SegmentCatalogReader for TableDirectory {
    fn get(&self, id: &str) -> StorageResult<Option<(usize, RecordSlot)>> {
        if self.control.fail_lookup.load(Ordering::Relaxed) {
            return Err(StorageError::invalid(
                "test directory",
                "injected lookup failure",
            ));
        }
        Ok(self.records.get(id).and_then(|(segment, slot)| {
            self.positions
                .get(segment)
                .map(|&position| (position, *slot))
        }))
    }

    fn position(&self, segment: SegmentId) -> StorageResult<Option<usize>> {
        Ok(self.positions.get(&segment).copied())
    }

    fn segments_for_file(&self, file: FileId) -> StorageResult<Cow<'_, [SegmentId]>> {
        Ok(Cow::Borrowed(
            self.files.get(&file.0).map_or(&[], Vec::as_slice),
        ))
    }

    fn estimated_directory_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }

    fn edit(&self) -> StorageResult<Box<dyn SegmentCatalogEditor>> {
        Ok(Box::new(self.clone()))
    }
}

impl SegmentCatalogEditor for TableDirectory {
    fn apply(&mut self, delta: SegmentDirectoryDelta<'_>) -> StorageResult<()> {
        match delta {
            SegmentDirectoryDelta::Install { source, position } => {
                let segment = source.segment_id();
                self.positions.insert(segment, position);
                source.visit_live_records(&mut |rows| {
                    for &(id, slot) in rows {
                        self.records.insert(id.clone(), (segment, slot));
                    }
                    Ok(())
                })?;
                source.visit_files(&mut |files| {
                    for id in files {
                        self.files.entry(id.0).or_default().push(segment);
                    }
                    Ok(())
                })?;
                self.control.installs.fetch_add(1, Ordering::Relaxed);
            }
            SegmentDirectoryDelta::RetireRecord(id) => {
                self.records.remove(id);
            }
            SegmentDirectoryDelta::RetireFile {
                file,
                segment,
                records,
            } => {
                for (id, slot) in records {
                    if self.records.get(id) == Some(&(segment, *slot)) {
                        self.records.remove(id);
                    }
                }
                if let Some(segments) = self.files.get_mut(&file.0) {
                    segments.retain(|&id| id != segment);
                }
            }
            SegmentDirectoryDelta::Replace {
                removed,
                replacement,
                positions,
            } => {
                for source in removed {
                    let segment = source.segment_id();
                    self.records.retain(|_, (owner, _)| *owner != segment);
                    self.files
                        .values_mut()
                        .for_each(|ids| ids.retain(|&owner| owner != segment));
                }
                self.positions = positions
                    .iter()
                    .copied()
                    .enumerate()
                    .map(|(position, id)| (id, position))
                    .collect();
                let position = self.positions[&replacement.segment_id()];
                self.apply(SegmentDirectoryDelta::Install {
                    source: replacement,
                    position,
                })?;
            }
        }
        Ok(())
    }

    fn seal(self: Box<Self>) -> StorageResult<SegmentCatalogHandle> {
        if self.control.fail_seal.swap(false, Ordering::Relaxed) {
            return Err(StorageError::invalid(
                "test directory",
                "injected seal failure",
            ));
        }
        Ok(Arc::from(self as Box<dyn SegmentCatalogReader>))
    }
}

#[test]
fn alternate_directory_is_used_for_live_ingestion_and_preserves_pinned_routes() {
    let factory = Arc::new(TableFactory::default());
    let mut builder = Index::builder();
    builder.use_storage_factory(Arc::new(Arc::clone(&factory)));
    let collection = builder.collection("entries", Schema::default()).unwrap();
    let registration = builder.register_records(collection, "fixture", 1).unwrap();
    let index = builder.build().unwrap();
    let session = index.ingestion();
    let writer = session.writer(registration).unwrap();

    let empty = index.view().unwrap();
    let mut rejected = writer.batch();
    rejected
        .insert("one", Value::String("original".into()))
        .unwrap();
    factory.fail_seal.store(true, Ordering::Relaxed);
    assert!(writer.submit(rejected).unwrap().wait_published().is_err());
    assert_eq!(factory.installs.load(Ordering::Relaxed), 1);
    assert_eq!(factory.worker_calls.load(Ordering::Relaxed), 1);
    let unchanged = index.view().unwrap();
    assert_eq!(unchanged.revision(), empty.revision());
    assert!(
        unchanged
            .get_record("entries", "one", ReturnMode::Locator)
            .unwrap()
            .is_none()
    );

    let retry_session = index.ingestion();
    let writer = retry_session.writer(registration).unwrap();
    let mut retry = writer.batch();
    retry
        .insert("one", Value::String("original".into()))
        .unwrap();
    writer.submit(retry).unwrap().wait_published().unwrap();
    assert_eq!(factory.worker_calls.load(Ordering::Relaxed), 2);
    let pinned = index.view().unwrap();
    assert!(
        pinned
            .get_record("entries", "one", ReturnMode::Locator)
            .unwrap()
            .is_some()
    );
    assert!(
        empty
            .get_record("entries", "one", ReturnMode::Locator)
            .unwrap()
            .is_none()
    );

    factory.fail_lookup.store(true, Ordering::Relaxed);
    assert!(
        pinned
            .get_record("entries", "one", ReturnMode::Locator)
            .unwrap_err()
            .contains("injected lookup failure")
    );
    factory.fail_lookup.store(false, Ordering::Relaxed);

    let mut delete = writer.batch();
    delete.delete("one").unwrap();
    writer.submit(delete).unwrap().wait_published().unwrap();
    index.compact().unwrap();
    assert!(
        index
            .view()
            .unwrap()
            .get_record("entries", "one", ReturnMode::Locator)
            .unwrap()
            .is_none()
    );
    assert!(
        pinned
            .get_record("entries", "one", ReturnMode::Locator)
            .unwrap()
            .is_some()
    );
}

#[test]
fn alternate_directory_keeps_managed_file_rows_and_routes_atomic() {
    use crate::repository::configuration::{CollectionDefinition, RepositoryOptions};
    use crate::source::test_support::{fixture_registry, line_format};

    let root = std::env::temp_dir().join(format!(
        "objekte-alternate-routes-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/first.rows"), b"first\nsecond\n").unwrap();
    let factory = Arc::new(TableFactory::default());
    let index = Index::start_build_with_factory(
        &root,
        vec![CollectionDefinition::folder(
            "entries",
            "data",
            line_format(),
        )],
        RepositoryOptions::default(),
        fixture_registry(),
        Arc::new(Arc::clone(&factory)),
    )
    .unwrap()
    .wait_complete()
    .unwrap();
    let pinned = index.view().unwrap();
    let first = pinned
        .file_catalog()
        .unwrap()
        .file_by_path(std::path::Path::new("data/first.rows"))
        .unwrap()
        .unwrap();
    assert_eq!(
        pinned
            .file_catalog()
            .unwrap()
            .records_for_file(first.id)
            .unwrap()
            .len(),
        2
    );

    let relative = std::path::Path::new("data/second.rows");
    std::fs::write(root.join(relative), b"third\nfourth\n").unwrap();
    let file_workers = factory.file_worker_calls.load(Ordering::Relaxed);
    assert!(file_workers > 0);
    factory.fail_file_worker.store(true, Ordering::Relaxed);
    let error = index.add_file(relative).unwrap_err();
    assert!(error.to_string().contains("injected prepared file failure"));
    assert!(factory.file_worker_calls.load(Ordering::Relaxed) > file_workers);
    assert_eq!(index.view().unwrap().revision(), pinned.revision());
    let installs = factory.installs.load(Ordering::Relaxed);
    factory.fail_seal.store(true, Ordering::Relaxed);
    let error = index.add_file(relative).unwrap_err();
    assert!(error.to_string().contains("injected seal failure"));
    assert_eq!(factory.installs.load(Ordering::Relaxed), installs + 1);
    let unchanged = index.view().unwrap();
    assert_eq!(unchanged.revision(), pinned.revision());
    assert!(
        unchanged
            .file_catalog()
            .unwrap()
            .file_by_path(relative)
            .unwrap()
            .is_none()
    );

    let added = index.add_file(relative).unwrap();
    let current = index.view().unwrap();
    let catalog = current.file_catalog().unwrap();
    assert_eq!(
        catalog.file_by_path(relative).unwrap().unwrap().id,
        added.file.file_id
    );
    let records = catalog.records_for_file(added.file.file_id).unwrap();
    assert_eq!(records.len(), 2);
    assert!(
        pinned
            .file_catalog()
            .unwrap()
            .file_by_path(relative)
            .unwrap()
            .is_none()
    );
    index.delete_file(added.file).unwrap();
    index.compact().unwrap();
    assert!(
        index
            .view()
            .unwrap()
            .file_catalog()
            .unwrap()
            .file_by_path(relative)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        catalog.records_for_file(added.file.file_id).unwrap(),
        records
    );
    std::fs::remove_dir_all(root).unwrap();
}
