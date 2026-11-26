use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::*;
use crate::construction::batch::BatchContribution;
use crate::index::storage::StorageFactory;
use crate::index::storage::liveness::{ActiveRecords, InitialLiveness, LiveSlotReader};
use crate::index::storage::order::{
    DeclaredOrderRows, DeclaredOrderStore, InferredOrderRows, OrderColumnReader, SortConfig,
    SortKey, SortOptions,
};
use crate::index::storage::ordered_slots::OrderedSlots;
use crate::index::storage::publication::PublishedSegment;
use crate::index::storage::segment::build::{
    BuiltSegment, DirectSegmentBuild, FileSegmentBuild, SegmentBuilder, SegmentSeal,
};
use crate::index::storage::segment::{NativeSegmentSummary, SegmentReader, SegmentResourceUsage};
use crate::index::storage::transfer::{BatchLimit, BatchStatus, StorageError};
use crate::lifecycle::Index;
use crate::model::{FieldPath, LogicalType, Value};
use crate::query::options::ReturnMode;
use crate::query_access::membership::MembershipView;
use crate::repository::statistics::AdmissionBreakdown;
use crate::schema::Schema;

#[path = "tests/segments.rs"]
mod segments;

#[derive(Default)]
struct TableFactory {
    independent_segments: AtomicBool,
    partial_score: AtomicBool,
    segment_id_lookups: AtomicUsize,
    origins_until_failure: AtomicUsize,
    independent_applications: AtomicUsize,
    text_applications: AtomicUsize,
    text_candidates: AtomicUsize,
    independent_projections: AtomicUsize,
    omit_projection: AtomicBool,

    installs: AtomicUsize,
    retained_builds: AtomicUsize,
    retained_reads: Arc<AtomicUsize>,
    fail_retained_read: Arc<AtomicBool>,
    order_builds: AtomicUsize,
    store_declared_order: AtomicBool,
    declared_order_builds: AtomicUsize,
    key_batches: Arc<AtomicUsize>,
    fail_key_transfer: Arc<AtomicBool>,
    order_rows: Arc<AtomicUsize>,
    fail_order_refill: Arc<AtomicBool>,
    liveness_visits: Arc<AtomicUsize>,
    worker_calls: AtomicUsize,
    file_worker_calls: AtomicUsize,
    direct_seals: AtomicUsize,
    file_batches_admitted: AtomicUsize,
    file_segments_sealed: AtomicUsize,
    fail_file_segment_seal: AtomicBool,
    fail_file_worker: AtomicBool,
    fail_seal: AtomicBool,
    fail_lookup: AtomicBool,
    fail_placement: AtomicBool,
    placement_reads: AtomicUsize,
    wrong_slot: AtomicBool,
    fail_directory_refill: AtomicBool,
}

impl DeclaredOrderStore for Arc<TableFactory> {
    fn accepts(&self, path: &FieldPath, kind: &LogicalType) -> bool {
        path.segments == ["score"] && kind == &LogicalType::Number
    }

    fn seal(&self, rows: DeclaredOrderRows) -> StorageResult<Arc<dyn OrderColumnReader>> {
        self.declared_order_builds.fetch_add(1, Ordering::Relaxed);
        Ok(Arc::new(TableOrder {
            path: rows.path,
            kind: rows.kind,
            collation_version: rows.collation_version,
            config: rows.config,
            keys: rows.keys.into_iter().enumerate().collect(),
            key_batches: Arc::clone(&self.key_batches),
            fail_key_transfer: Arc::clone(&self.fail_key_transfer),
            directions: [
                Some(self.short_order(rows.ascending)),
                Some(self.short_order(rows.descending)),
            ],
        }))
    }
}

impl TableFactory {
    fn short_order(&self, rows: Vec<RecordSlot>) -> ShortOrder {
        ShortOrder {
            blocks: rows
                .chunks(2)
                .map(|block| [block[0].0, block.get(1).map_or(0, |slot| slot.0)])
                .collect(),
            len: rows.len(),
            read_rows: Arc::clone(&self.order_rows),
            fail_refill: Arc::clone(&self.fail_order_refill),
        }
    }
}

impl StorageFactory for Arc<TableFactory> {
    fn import_file_segment(
        &self,
        source: &dyn SegmentReader,
        layout: crate::index::storage::segment::transfer::FileSegmentLayout<'_>,
        cancelled: &mut dyn FnMut() -> bool,
    ) -> StorageResult<Option<crate::index::storage::segment::SegmentHandle>> {
        segments::import_file_segment(source, layout, cancelled, Arc::clone(self))
    }

    fn project_segment_partitions(
        &self,
        source: &dyn SegmentReader,
        partitions: &[crate::index::storage::segment::maintenance::SegmentPartition],
        files: &dyn crate::index::storage::file_catalog::FileCatalogReader,
        cancelled: &mut dyn FnMut() -> bool,
    ) -> StorageResult<Vec<crate::index::storage::segment::SegmentHandle>> {
        let mut projected = crate::index::storage::test_factory()
            .project_segment_partitions(source, partitions, files, cancelled)?;
        if self.omit_projection.swap(false, Ordering::Relaxed) {
            projected.pop();
        }
        Ok(projected)
    }

    fn consolidate_segments(
        &self,
        segments: &[&dyn SegmentReader],
        max_records: usize,
    ) -> StorageResult<Option<crate::index::storage::segment::maintenance::ConsolidatedSegment>>
    {
        crate::index::storage::test_factory().consolidate_segments(segments, max_records)
    }

    fn base_segment(
        &self,
        collection: &crate::repository::state::CollectionState,
        files: &dyn crate::index::storage::file_catalog::FileCatalogReader,
    ) -> StorageResult<crate::index::storage::segment::SegmentHandle> {
        crate::index::storage::test_factory().base_segment(collection, files)
    }

    fn declared_order_store(&self) -> Option<Arc<dyn DeclaredOrderStore>> {
        self.store_declared_order
            .load(Ordering::Relaxed)
            .then(|| Arc::new(Arc::clone(self)) as Arc<dyn DeclaredOrderStore>)
    }

    fn start_file_segment(
        &self,
        input: FileSegmentBuild<'_>,
    ) -> StorageResult<Box<dyn SegmentBuilder>> {
        if self.independent_segments.load(Ordering::Relaxed) {
            return Ok(Box::new(segments::TableBuilder::new(
                input,
                Arc::clone(self),
            )));
        }
        Ok(Box::new(FailureInjectingBuilder {
            inner: crate::index::storage::default_factory().start_file_segment(input)?,
            control: Arc::clone(self),
        }))
    }

    fn seal_direct_segment(
        &self,
        input: DirectSegmentBuild<'_>,
        seal: SegmentSeal<'_>,
    ) -> StorageResult<BuiltSegment> {
        self.direct_seals.fetch_add(1, Ordering::Relaxed);
        if self.independent_segments.load(Ordering::Relaxed) {
            segments::seal_direct(input, seal, Arc::clone(self))
        } else {
            crate::index::storage::default_factory().seal_direct_segment(input, seal)
        }
    }

    fn retain_record_bodies(
        &self,
        bodies: Vec<crate::records::RetainedBody>,
    ) -> StorageResult<crate::index::storage::segment::RetainedBlock> {
        self.retained_builds.fetch_add(1, Ordering::Relaxed);
        let count = bodies.len();
        Ok(Arc::new(TableBodies {
            count,
            bodies: bodies.into_iter().enumerate().collect(),
            reads: Arc::clone(&self.retained_reads),
            fail_read: Arc::clone(&self.fail_retained_read),
        }))
    }

    fn inferred_order(&self, rows: InferredOrderRows) -> StorageResult<Arc<dyn OrderColumnReader>> {
        self.order_builds.fetch_add(1, Ordering::Relaxed);
        let mut directions = [None, None];
        directions[usize::from(rows.descending)] = Some(self.short_order(rows.order));
        Ok(Arc::new(TableOrder {
            path: rows.path,
            kind: rows.kind,
            collation_version: crate::index::storage::order::COLLATION_VERSION.into(),
            config: rows.config,
            keys: rows.keys.into_iter().enumerate().collect(),
            key_batches: Arc::clone(&self.key_batches),
            fail_key_transfer: Arc::clone(&self.fail_key_transfer),
            directions,
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
        let contribution = if self.independent_segments.load(Ordering::Relaxed) {
            let worker = segments::TableWorker::new(&batch.definition.schema);
            crate::construction::batch::prepare_file_records(batch, worker)?
        } else {
            crate::index::storage::default_factory().prepare_file_records(batch)?
        };
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
        if self.independent_segments.load(Ordering::Relaxed) {
            let worker = segments::TableWorker::new(&batch.declaration.schema);
            crate::construction::batch::prepare_direct_records(batch, worker)
        } else {
            crate::index::storage::default_factory().prepare_direct_records(batch)
        }
    }

    fn start_file_discovery(
        &self,
        root: std::path::PathBuf,
    ) -> StorageResult<Box<dyn crate::index::storage::file_catalog::FileDiscoveryBuilder>> {
        crate::index::storage::test_factory().start_file_discovery(root)
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
            descriptors: BTreeMap::new(),
            placements: BTreeMap::new(),
            control: Arc::clone(self),
        }))
    }
}

struct FailureInjectingBuilder {
    inner: Box<dyn SegmentBuilder>,
    control: Arc<TableFactory>,
}

impl SegmentBuilder for FailureInjectingBuilder {
    fn file_count(&self) -> usize {
        self.inner.file_count()
    }

    fn record_count(&self) -> usize {
        self.inner.record_count()
    }

    fn source_bytes(&self) -> u64 {
        self.inner.source_bytes()
    }

    fn at_capacity(&self) -> bool {
        self.inner.at_capacity()
    }

    fn can_admit(&self, batch: &BatchContribution) -> StorageResult<bool> {
        self.inner.can_admit(batch)
    }

    fn admit(
        &mut self,
        batch: BatchContribution,
        profile: Option<&mut AdmissionBreakdown>,
    ) -> StorageResult<()> {
        self.inner.admit(batch, profile)?;
        self.control
            .file_batches_admitted
            .fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn seal(self: Box<Self>, input: SegmentSeal<'_>) -> StorageResult<BuiltSegment> {
        let built = self.inner.seal(input)?;
        self.control
            .file_segments_sealed
            .fetch_add(1, Ordering::Relaxed);
        if self
            .control
            .fail_file_segment_seal
            .swap(false, Ordering::Relaxed)
        {
            return Err(StorageError::invalid(
                "test file segment",
                "injected segment seal failure",
            ));
        }
        Ok(built)
    }
}

struct TableBodies {
    count: usize,
    bodies: BTreeMap<usize, crate::records::RetainedBody>,
    reads: Arc<AtomicUsize>,
    fail_read: Arc<AtomicBool>,
}

impl crate::index::storage::segment::RetainedBlockReader for TableBodies {
    fn len(&self) -> usize {
        self.count
    }

    fn body(&self, offset: u32) -> StorageResult<Cow<'_, crate::records::RetainedBody>> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        if self.fail_read.load(Ordering::Relaxed) {
            return Err(StorageError::invalid(
                "read test body",
                "injected retained block failure",
            ));
        }
        self.bodies
            .get(&(offset as usize))
            .cloned()
            .map(Cow::Owned)
            .ok_or_else(|| StorageError::invalid("read test body", "offset unavailable"))
    }

    fn estimated_body_bytes(&self) -> usize {
        self.bodies.values().fold(0, |total, body| {
            total
                .saturating_add(crate::ingestion::estimated_value_bytes(&body.value))
                .saturating_add(crate::ingestion::estimated_text_bytes(&body.text))
        })
    }
}

#[test]
fn retained_blocks_survive_publication_and_source_free_slot_remapping() {
    use crate::query_results::Payload;

    let factory = Arc::new(TableFactory::default());
    let mut builder = Index::builder();
    builder.use_storage_factory(Arc::new(Arc::clone(&factory)));
    let collection = builder.collection("entries", Schema::default()).unwrap();
    let registration = builder
        .register_records(collection, "fixture-bodies", 1)
        .unwrap();
    let index = builder.build().unwrap();
    let session = index.ingestion();
    let writer = session.writer(registration).unwrap();
    let mut insert = writer.batch();
    insert
        .insert("removed", Value::String("old".into()))
        .unwrap();
    let kept = Value::String("kept".into());
    insert.insert("kept", kept.clone()).unwrap();
    writer.submit(insert).unwrap().wait_published().unwrap();
    let pinned = index.view().unwrap();
    let write_version = |view: &crate::lifecycle::IndexView| {
        let found = view.0.record_routes[0].locate("kept").unwrap().unwrap();
        view.0.segment_group(0).unwrap()[found.position]
            .index
            .write_version(found.route.slot)
            .unwrap()
            .unwrap()
    };
    let first_write = write_version(&pinned);
    let before = pinned.0.record_routes[0].route("kept").unwrap().unwrap();
    assert_eq!(before.slot, RecordSlot(1));
    assert_eq!(factory.retained_builds.load(Ordering::Relaxed), 1);
    assert_eq!(factory.direct_seals.load(Ordering::Relaxed), 1);
    assert_eq!(factory.retained_reads.load(Ordering::Relaxed), 0);

    factory.fail_placement.store(true, Ordering::Relaxed);
    assert!(
        pinned
            .get_record("entries", "kept", ReturnMode::Locator)
            .unwrap_err()
            .contains("injected placement failure")
    );
    factory.fail_placement.store(false, Ordering::Relaxed);
    factory.wrong_slot.store(true, Ordering::Relaxed);
    assert!(
        pinned
            .get_record("entries", "kept", ReturnMode::Locator)
            .unwrap_err()
            .contains("points to a different or missing record")
    );
    factory.wrong_slot.store(false, Ordering::Relaxed);

    factory.fail_retained_read.store(true, Ordering::Relaxed);
    assert!(
        pinned
            .get_record("entries", "kept", ReturnMode::Locator)
            .unwrap()
            .is_some()
    );
    assert_eq!(factory.retained_reads.load(Ordering::Relaxed), 0);
    assert!(
        pinned
            .get_record("entries", "kept", ReturnMode::Logical)
            .unwrap_err()
            .contains("injected retained block failure")
    );

    let mut delete = writer.batch();
    delete.delete("removed").unwrap();
    writer.submit(delete).unwrap().wait_published().unwrap();
    assert_eq!(index.compact().unwrap().retired_slots_removed, 1);
    assert_eq!(factory.retained_builds.load(Ordering::Relaxed), 1);
    assert_eq!(factory.retained_reads.load(Ordering::Relaxed), 1);
    let compacted = index.view().unwrap();
    let after = compacted.0.record_routes[0].route("kept").unwrap().unwrap();
    assert_ne!(after.segment, before.segment);
    assert_eq!(after.slot, RecordSlot(0));
    assert_eq!(write_version(&compacted), first_write);

    factory.fail_retained_read.store(false, Ordering::Relaxed);
    for view in [&pinned, &compacted] {
        assert!(matches!(
            view.get_record("entries", "kept", ReturnMode::Logical).unwrap().unwrap().payload,
            Some(Payload::Logical(value)) if value == kept
        ));
    }
    assert!(
        compacted
            .get_record("entries", "removed", ReturnMode::Locator)
            .unwrap()
            .is_none()
    );
    assert!(
        pinned
            .get_record("entries", "removed", ReturnMode::Locator)
            .unwrap()
            .is_some()
    );
    let mut replacement = writer.batch();
    replacement
        .replace("kept", Value::String("changed".into()))
        .unwrap();
    writer
        .submit(replacement)
        .unwrap()
        .wait_published()
        .unwrap();
    assert_ne!(write_version(&index.view().unwrap()), first_write);
    assert_eq!(write_version(&pinned), first_write);
}

struct TableOrder {
    path: FieldPath,
    kind: LogicalType,
    collation_version: String,
    config: SortConfig,
    keys: BTreeMap<usize, SortKey>,
    key_batches: Arc<AtomicUsize>,
    fail_key_transfer: Arc<AtomicBool>,
    directions: [Option<ShortOrder>; 2],
}

impl OrderColumnReader for TableOrder {
    fn path(&self) -> &FieldPath {
        &self.path
    }

    fn kind(&self) -> &LogicalType {
        &self.kind
    }

    fn collation_version(&self) -> &str {
        &self.collation_version
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
            + self
                .directions
                .iter()
                .flatten()
                .map(|order| order.blocks.capacity() * size_of::<[u32; 2]>())
                .sum::<usize>()
    }

    fn key_count(&self) -> usize {
        self.keys.len()
    }

    fn key(&self, slot: RecordSlot) -> StorageResult<Cow<'_, SortKey>> {
        self.keys
            .get(&(slot.0 as usize))
            .cloned()
            .map(Cow::Owned)
            .ok_or_else(|| StorageError::invalid("test order", "slot out of range"))
    }

    fn key_batch(
        &self,
        slots: &[RecordSlot],
        limit: BatchLimit,
    ) -> StorageResult<(Vec<Cow<'_, SortKey>>, BatchStatus)> {
        let batch = self.key_batches.fetch_add(1, Ordering::Relaxed);
        if batch != 0 && self.fail_key_transfer.load(Ordering::Relaxed) {
            return Err(StorageError::invalid(
                "read test key batch",
                "injected order transfer failure",
            ));
        }
        let keys = crate::index::storage::order::key_transfer::read_order_keys(
            self,
            &slots[..slots.len().min(2)],
            limit,
        )?;
        let status = if keys.len() == slots.len() {
            BatchStatus::Done
        } else {
            BatchStatus::More
        };
        Ok((keys, status))
    }

    fn slots(&self, descending: bool) -> StorageResult<&dyn OrderedSlots> {
        self.directions[usize::from(descending)]
            .as_ref()
            .map(|order| order as &dyn OrderedSlots)
            .ok_or_else(|| StorageError::invalid("test order", "order direction was not built"))
    }
}

struct ShortOrder {
    blocks: Vec<[u32; 2]>,
    len: usize,
    read_rows: Arc<AtomicUsize>,
    fail_refill: Arc<AtomicBool>,
}

impl OrderedSlots for ShortOrder {
    fn len(&self) -> usize {
        self.len
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
        if start > self.len {
            return Err(StorageError::invalid("test order", "start out of range"));
        }
        let end = self.len.min(start.saturating_add(size));
        let batch: Vec<_> = (start..end)
            .map(|position| RecordSlot(self.blocks[position / 2][position % 2]))
            .collect();
        self.read_rows.fetch_add(batch.len(), Ordering::Relaxed);
        let status = if end == self.len {
            BatchStatus::Done
        } else {
            BatchStatus::More
        };
        Ok((Cow::Owned(batch), status))
    }
}

#[test]
fn native_postings_and_an_independent_declared_order_share_a_segment() {
    use crate::query::options::SortField;
    use crate::query::{Query, QueryOptions, parse_filter};
    use crate::repository::configuration::RepositoryOptions;
    use crate::schema::FieldDefinition;
    use crate::source::test_support::{document_folder, fixture_registry};

    let root = std::env::temp_dir().join(format!("objekte-declared-order-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for n in 0..64 {
        std::fs::write(
            root.join(format!("data/{n:03}.row")),
            format!(r#"("score"={n})"#),
        )
        .unwrap();
    }
    let factory = Arc::new(TableFactory::default());
    factory.store_declared_order.store(true, Ordering::Relaxed);
    let mut definition = document_folder("entries", "data");
    definition
        .schema
        .register_field(FieldDefinition {
            path: vec!["score".into()],
            kind: LogicalType::Number,
            required: true,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    definition
        .sort_fields
        .push(FieldPath::parse("score").unwrap());
    let build = |storage: Arc<dyn StorageFactory>| {
        Index::start_build_with_factory(
            &root,
            vec![definition.clone()],
            RepositoryOptions {
                verified_cache_bytes: 0,
                ..Default::default()
            },
            fixture_registry(),
            storage,
        )
        .unwrap()
        .wait_complete()
        .unwrap()
    };
    let mixed = build(Arc::new(Arc::clone(&factory)));
    let memory = build(crate::index::storage::default_factory());
    let pinned = mixed.view().unwrap();
    let memory_before = memory.view().unwrap();
    let mut query = Query {
        collections: vec!["entries".into()],
        filter: Some(parse_filter("score >= 10").unwrap()),
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
    assert_eq!(factory.declared_order_builds.load(Ordering::Relaxed), 1);
    let result = pinned.search(&query).unwrap();
    assert_eq!(result.hits, memory_before.search(&query).unwrap().hits);
    assert_eq!(result.hits[0].record_id, "path:063.row");
    assert!(result.counters.exact_posting_matches > 0);
    assert_eq!(result.counters.record_parses, 0);
    assert_eq!(factory.order_rows.load(Ordering::Relaxed), 2);
    assert_eq!(
        pinned.native_index_signature().unwrap(),
        memory_before.native_index_signature().unwrap()
    );

    query.options.limit = 4;
    factory.fail_order_refill.store(true, Ordering::Relaxed);
    assert!(
        pinned
            .search(&query)
            .unwrap_err()
            .contains("injected refill failure")
    );
    factory.fail_order_refill.store(false, Ordering::Relaxed);
    for index in [&mixed, &memory] {
        let view = index.view().unwrap();
        let file = view
            .file_catalog()
            .unwrap()
            .file_by_path(std::path::Path::new("data/063.row"))
            .unwrap()
            .unwrap();
        let handle = view.file_handle(file.id).unwrap().unwrap();
        index.delete_file(handle).unwrap();
    }
    std::fs::remove_dir_all(root).unwrap();
    let before_compact = mixed.view().unwrap().revision();
    factory.fail_key_transfer.store(true, Ordering::Relaxed);
    let error = mixed.compact().unwrap_err();
    assert!(
        error
            .to_string()
            .contains("injected order transfer failure")
    );
    assert_eq!(factory.key_batches.load(Ordering::Relaxed), 2);
    assert_eq!(mixed.view().unwrap().revision(), before_compact);
    factory.fail_key_transfer.store(false, Ordering::Relaxed);
    for index in [&mixed, &memory] {
        assert_eq!(index.compact().unwrap().retired_slots_removed, 1);
    }
    assert_eq!(factory.declared_order_builds.load(Ordering::Relaxed), 2);
    assert_eq!(factory.key_batches.load(Ordering::Relaxed), 34);
    let mixed_after = mixed.view().unwrap();
    let memory_after = memory.view().unwrap();
    assert_eq!(
        mixed_after.search(&query).unwrap().hits,
        memory_after.search(&query).unwrap().hits
    );
    assert_eq!(
        mixed_after.native_index_signature().unwrap(),
        memory_after.native_index_signature().unwrap()
    );
    assert_eq!(
        pinned.search(&query).unwrap().hits[0].record_id,
        "path:063.row"
    );
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
    descriptors: BTreeMap<usize, SegmentDescriptor>,
    placements: BTreeMap<SegmentId, Arc<PublishedSegment>>,
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
        Ok(self.records.get(id).map(|&(segment, slot)| RoutedSlot {
            segment,
            slot: if self.control.wrong_slot.load(Ordering::Relaxed) {
                RecordSlot(0)
            } else {
                slot
            },
        }))
    }

    fn segment_count(&self) -> usize {
        self.descriptors.len()
    }

    fn read_descriptors(
        &self,
        start: usize,
        limit: BatchLimit,
    ) -> StorageResult<(Cow<'_, [SegmentDescriptor]>, BatchStatus)> {
        if start > 0 && self.control.fail_directory_refill.load(Ordering::Relaxed) {
            return Err(StorageError::invalid(
                "table directory",
                "injected descriptor refill failure",
            ));
        }
        assert!(limit.accepts("table directory", 0, 0, size_of::<SegmentDescriptor>())?);
        let rows = self
            .descriptors
            .get(&start)
            .copied()
            .into_iter()
            .collect::<Vec<_>>();
        let status = if start + rows.len() == self.descriptors.len() {
            BatchStatus::Done
        } else {
            BatchStatus::More
        };
        Ok((Cow::Owned(rows), status))
    }

    fn position(&self, segment: SegmentId) -> StorageResult<Option<usize>> {
        Ok(self.positions.get(&segment).copied())
    }

    fn placement_at(
        &self,
        position: usize,
    ) -> StorageResult<Option<Cow<'_, Arc<PublishedSegment>>>> {
        self.control.placement_reads.fetch_add(1, Ordering::Relaxed);
        if self.control.fail_placement.load(Ordering::Relaxed) {
            return Err(StorageError::invalid(
                "pin test placement",
                "injected placement failure",
            ));
        }
        Ok(self
            .descriptors
            .get(&position)
            .and_then(|descriptor| self.placements.get(&descriptor.id))
            .cloned()
            .map(Cow::Owned))
    }

    fn read_placements(
        &self,
        start: usize,
        limit: BatchLimit,
    ) -> StorageResult<(Cow<'_, [Arc<PublishedSegment>]>, BatchStatus)> {
        assert!(limit.accepts("table directory", 0, 0, size_of::<Arc<PublishedSegment>>())?);
        if start == self.segment_count() {
            return Ok((Cow::Borrowed(&[]), BatchStatus::Done));
        }
        let placements = self
            .placement_at(start)?
            .map(Cow::into_owned)
            .into_iter()
            .collect::<Vec<_>>();
        let status = if start + placements.len() == self.segment_count() {
            BatchStatus::Done
        } else {
            BatchStatus::More
        };
        Ok((Cow::Owned(placements), status))
    }

    fn record_counts(&self) -> SegmentRecordCounts {
        SegmentRecordCounts::for_segments(
            self.descriptors
                .values()
                .filter_map(|descriptor| self.placements.get(&descriptor.id)),
        )
    }

    fn estimates(&self) -> DirectoryEstimates {
        DirectoryEstimates::for_segments(
            self.descriptors
                .values()
                .filter_map(|descriptor| self.placements.get(&descriptor.id)),
        )
    }

    fn visit_summaries(&self, visit: &mut dyn FnMut(NativeSegmentSummary)) {
        for segment in self
            .descriptors
            .values()
            .filter_map(|descriptor| self.placements.get(&descriptor.id))
        {
            visit(segment.summary(segment.active.len() as usize));
        }
    }

    fn visit_index_families(
        &self,
        visit: &mut dyn FnMut(u64, crate::lifecycle::IndexFamilyEstimate),
    ) {
        for segment in self
            .descriptors
            .values()
            .filter_map(|descriptor| self.placements.get(&descriptor.id))
        {
            visit(segment.id.get(), segment.index_family_estimate());
        }
    }

    fn observe_backing(&self, usage: &mut SegmentResourceUsage) {
        for segment in self
            .descriptors
            .values()
            .filter_map(|descriptor| self.placements.get(&descriptor.id))
        {
            segment.observe_backing(usage);
        }
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
                let segment = source.id;
                self.positions.insert(segment, position);
                self.placements.insert(segment, Arc::clone(source));
                self.descriptors.insert(position, source.descriptor());
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
            SegmentDirectoryDelta::RetireRecord { id, placement } => {
                self.records.remove(id);
                self.placements.insert(placement.id, Arc::clone(placement));
            }
            SegmentDirectoryDelta::RefreshPlacement(placement) => {
                self.placements.insert(placement.id, Arc::clone(placement));
            }
            SegmentDirectoryDelta::RetireFile {
                file,
                segment,
                records,
                placement,
            } => {
                self.placements.insert(segment, Arc::clone(placement));
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
                    let segment = source.id;
                    self.placements.remove(&segment);
                    self.records.retain(|_, (owner, _)| *owner != segment);
                    self.files
                        .values_mut()
                        .for_each(|ids| ids.retain(|&owner| owner != segment));
                }
                let next_descriptors = positions
                    .iter()
                    .enumerate()
                    .filter(|(_, id)| **id != replacement.id)
                    .map(|(position, id)| (position, self.descriptors[&self.positions[id]]))
                    .collect();
                self.descriptors = next_descriptors;
                self.positions = positions
                    .iter()
                    .copied()
                    .enumerate()
                    .map(|(position, id)| (id, position))
                    .collect();
                let position = self.positions[&replacement.id];
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
    let admitted = factory.file_batches_admitted.load(Ordering::Relaxed);
    let sealed = factory.file_segments_sealed.load(Ordering::Relaxed);
    assert!(admitted > 0);
    assert!(sealed > 0);
    factory
        .fail_file_segment_seal
        .store(true, Ordering::Relaxed);
    let error = index.add_file(relative).unwrap_err();
    assert!(error.to_string().contains("injected segment seal failure"));
    assert_eq!(index.view().unwrap().revision(), pinned.revision());
    assert!(factory.file_batches_admitted.load(Ordering::Relaxed) > admitted);
    assert!(factory.file_segments_sealed.load(Ordering::Relaxed) > sealed);
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
    let mut summaries = 0;
    factory.fail_directory_refill.store(true, Ordering::Relaxed);
    let error = current
        .index_observation_with_segments(|_| summaries += 1)
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("injected descriptor refill failure")
    );
    assert_eq!(summaries, 1);
    factory
        .fail_directory_refill
        .store(false, Ordering::Relaxed);
    assert_eq!(current.index_observation().unwrap().records, 4);
    assert_eq!(pinned.index_observation().unwrap().records, 2);
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
