use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::*;
use crate::index::storage::StorageFactory;
use crate::index::storage::liveness::{ActiveRecords, InitialLiveness, LiveSlotReader};
use crate::index::storage::order::{
    InferredOrderRows, OrderColumnReader, SortConfig, SortKey, SortOptions,
};
use crate::index::storage::ordered_slots::OrderedSlots;
use crate::index::storage::transfer::{BatchLimit, BatchStatus, StorageError};
use crate::lifecycle::Index;
use crate::model::{FieldPath, Value};
use crate::query::options::ReturnMode;
use crate::query_access::membership::MembershipView;
use crate::schema::Schema;

#[derive(Default)]
struct TableFactory {
    installs: AtomicUsize,
    order_builds: AtomicUsize,
    order_rows: Arc<AtomicUsize>,
    fail_order_refill: Arc<AtomicBool>,
    liveness_visits: Arc<AtomicUsize>,
    worker_calls: AtomicUsize,
    file_worker_calls: AtomicUsize,
    fail_file_worker: AtomicBool,
    fail_seal: AtomicBool,
    fail_lookup: AtomicBool,
}

impl StorageFactory for Arc<TableFactory> {
    fn inferred_order(&self, rows: InferredOrderRows) -> StorageResult<Arc<dyn OrderColumnReader>> {
        self.order_builds.fetch_add(1, Ordering::Relaxed);
        let keys = rows.keys.into_iter().enumerate().collect();
        Ok(Arc::new(TableOrder {
            path: rows.path,
            config: rows.config,
            keys,
            descending: rows.descending,
            slots: ShortOrder {
                rows: rows.order,
                read_rows: Arc::clone(&self.order_rows),
                fail_refill: Arc::clone(&self.fail_order_refill),
            },
        }))
    }

    fn live_records(
        &self,
        slots: usize,
        initial: InitialLiveness<'_>,
    ) -> StorageResult<ActiveRecords> {
        let count = u32::try_from(slots)
            .map_err(|_| StorageError::ResourceLimit("too many test slots".into()))?;
        let live = match initial {
            InitialLiveness::All => (0..count).collect(),
            InitialLiveness::Selected(members) => members.iter().collect(),
        };
        Ok(ActiveRecords::new(SortedLiveSlots {
            count,
            live,
            visits: Arc::clone(&self.liveness_visits),
        }))
    }

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

struct TableOrder {
    path: FieldPath,
    config: SortConfig,
    keys: BTreeMap<usize, SortKey>,
    descending: bool,
    slots: ShortOrder,
}

impl OrderColumnReader for TableOrder {
    fn path(&self) -> &FieldPath {
        &self.path
    }

    fn options(&self) -> SortOptions {
        self.config.options
    }

    fn locale(&self) -> &str {
        &self.config.locale
    }

    fn retained_bytes(&self) -> usize {
        size_of::<Self>()
            + self.keys.len() * size_of::<(usize, SortKey)>()
            + self.slots.rows.capacity() * size_of::<RecordSlot>()
    }

    fn key(&self, slot: RecordSlot) -> StorageResult<Cow<'_, SortKey>> {
        self.keys
            .get(&(slot.0 as usize))
            .cloned()
            .map(Cow::Owned)
            .ok_or_else(|| StorageError::invalid("test order", "slot out of range"))
    }

    fn slots(&self, descending: bool) -> StorageResult<&dyn OrderedSlots> {
        if descending != self.descending {
            return Err(StorageError::invalid(
                "test order",
                "order direction was not built",
            ));
        }
        Ok(&self.slots)
    }
}

struct ShortOrder {
    rows: Vec<RecordSlot>,
    read_rows: Arc<AtomicUsize>,
    fail_refill: Arc<AtomicBool>,
}

impl OrderedSlots for ShortOrder {
    fn len(&self) -> usize {
        self.rows.len()
    }

    fn read_batch(
        &self,
        start: usize,
        limit: BatchLimit,
    ) -> StorageResult<(Cow<'_, [RecordSlot]>, BatchStatus)> {
        let size = limit
            .fixed_batch_len("test order", size_of::<RecordSlot>())?
            .min(2);
        if start > 0 && self.fail_refill.load(Ordering::Relaxed) {
            return Err(StorageError::invalid(
                "test order",
                "injected refill failure",
            ));
        }
        let remaining = self
            .rows
            .get(start..)
            .ok_or_else(|| StorageError::invalid("test order", "start out of range"))?;
        let batch = &remaining[..size.min(remaining.len())];
        self.read_rows.fetch_add(batch.len(), Ordering::Relaxed);
        let status = if batch.len() == remaining.len() {
            BatchStatus::Done
        } else {
            BatchStatus::More
        };
        Ok((Cow::Owned(batch.to_vec()), status))
    }
}

#[test]
fn inferred_order_reads_independent_short_batches_through_search() {
    use crate::inference::{InferenceConfig, InferenceMode};
    use crate::query::options::SortField;
    use crate::query::{Query, QueryOptions};
    use crate::repository::configuration::RepositoryOptions;
    use crate::source::test_support::{document_folder, fixture_registry};

    let root = std::env::temp_dir().join(format!(
        "objekte-independent-inferred-order-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let data = root.join("data");
    for score in 0..64 {
        std::fs::write(
            data.join(format!("{score:02}.row")),
            format!(r#"("score"={score})"#),
        )
        .unwrap();
    }
    let build = |factory| {
        Index::start_build_with_factory(
            &root,
            vec![document_folder("data", "data")],
            RepositoryOptions {
                verified_cache_bytes: 0,
                inference: InferenceConfig {
                    mode: InferenceMode::AutoIndex,
                    ..Default::default()
                },
                ..Default::default()
            },
            fixture_registry(),
            factory,
        )
        .unwrap()
        .wait_complete()
        .unwrap()
        .view()
        .unwrap()
    };
    let factory = Arc::new(TableFactory::default());
    let table = build(Arc::new(Arc::clone(&factory)));
    let memory = build(crate::index::storage::default_factory());
    let mut query = Query {
        collections: vec!["data".into()],
        filter: None,
        fts: None,
        options: QueryOptions {
            count: false,
            limit: 1,
            return_mode: ReturnMode::Locator,
            evidence: false,
            sort: vec![SortField::simple("score", true)],
            offset: 0,
        },
    };
    let expected = memory.search(&query).unwrap();
    let result = table.search(&query).unwrap();
    assert_eq!(result.hits, expected.hits);
    assert_eq!(result.hits.len(), 1);
    assert_eq!(result.counters.files_opened, 0);
    assert_eq!(factory.order_builds.load(Ordering::Relaxed), 1);
    assert_eq!(factory.order_rows.load(Ordering::Relaxed), 2);

    query.options.limit = 5;
    factory.fail_order_refill.store(true, Ordering::Relaxed);
    assert!(
        table
            .search(&query)
            .unwrap_err()
            .to_string()
            .contains("injected refill failure")
    );
    factory.fail_order_refill.store(false, Ordering::Relaxed);
    assert_eq!(
        table.search(&query).unwrap().hits,
        memory.search(&query).unwrap().hits
    );
    assert_eq!(factory.order_builds.load(Ordering::Relaxed), 1);
    std::fs::remove_dir_all(root).unwrap();
}

#[derive(Clone, Debug)]
struct SortedLiveSlots {
    count: u32,
    live: Vec<u32>,
    visits: Arc<AtomicUsize>,
}

impl LiveSlotReader for SortedLiveSlots {
    fn slot_count(&self) -> u32 {
        self.count
    }

    fn live_count(&self) -> u64 {
        self.live.len() as u64
    }

    fn contains(&self, slot: RecordSlot) -> bool {
        self.live.binary_search(&slot.0).is_ok()
    }

    fn visit_blocks(&self, visit: &mut dyn FnMut(u32, MembershipView<'_>)) {
        self.visits.fetch_add(1, Ordering::Relaxed);
        for rows in self.live.chunks(2) {
            visit(0, MembershipView::Sorted(rows));
        }
    }

    fn retired(&self, slots: &[RecordSlot]) -> StorageResult<ActiveRecords> {
        let mut next = self.clone();
        for slot in slots {
            if slot.0 >= self.count {
                return Err(StorageError::invalid(
                    "retire test slots",
                    "slot out of range",
                ));
            }
            if let Ok(position) = next.live.binary_search(&slot.0) {
                next.live.remove(position);
            }
        }
        Ok(ActiveRecords::new(next))
    }
}

#[test]
fn selected_slot_membership_keeps_sparse_revisions_queryable() {
    let factory = Arc::new(TableFactory::default());
    let mut builder = Index::builder();
    builder.use_storage_factory(Arc::new(Arc::clone(&factory)));
    let collection = builder.collection("entries", Schema::default()).unwrap();
    let registration = builder
        .register_records(collection, "fixture-liveness", 1)
        .unwrap();
    let index = builder.build().unwrap();
    let session = index.ingestion();
    let writer = session.writer(registration).unwrap();
    let mut input = writer.batch();
    for id in ["one", "two", "three"] {
        input.insert(id, Value::String(id.into())).unwrap();
    }
    writer.submit(input).unwrap().wait_published().unwrap();
    let pinned = index.view().unwrap();
    let mut deletion = writer.batch();
    deletion.delete("two").unwrap();
    writer.submit(deletion).unwrap().wait_published().unwrap();
    let current = index.view().unwrap();
    let query = crate::query::Query {
        collections: vec!["entries".into()],
        filter: None,
        fts: None,
        options: crate::query::QueryOptions {
            count: true,
            limit: 10,
            return_mode: ReturnMode::Locator,
            evidence: false,
            sort: Vec::new(),
            offset: 0,
        },
    };
    let old = pinned.search(&query).unwrap();
    assert_eq!(old.count, Some(3));
    let live = current.search(&query).unwrap();
    assert_eq!(live.count, Some(2));
    assert_eq!(
        live.hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["one", "three"]
    );
    assert!(factory.liveness_visits.load(Ordering::Relaxed) > 0);
    assert_eq!(pinned.search(&query).unwrap().count, Some(3));
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
    fn route(&self, id: &str) -> StorageResult<Option<RoutedSlot>> {
        if self.control.fail_lookup.load(Ordering::Relaxed) {
            return Err(StorageError::invalid(
                "test directory",
                "injected lookup failure",
            ));
        }
        Ok(self
            .records
            .get(id)
            .map(|&(segment, slot)| RoutedSlot { segment, slot }))
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
