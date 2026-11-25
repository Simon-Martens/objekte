use std::borrow::Cow;
use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::ops::RangeBounds;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::FileCatalog;
use crate::index::storage::file_catalog::{
    FileCatalogBase, FileCatalogDelta, FileCatalogEditor, FileCatalogHandle, FileCatalogReader,
    FileCatalogRevision, FileCatalogSource, FilePathBounds, FileScalarPredicate, FileTransferRow,
    transfer_files,
};
use crate::index::storage::transfer::{BatchLimit, BatchStatus, StorageError, StorageResult};

/// An independent catalog used in transfer tests. Store only ordered live
/// rows, with no native inventory, index, masks, or lookup tables.
#[derive(Clone)]
struct OrderedFiles {
    allocated: u32,
    rows: Vec<FileTransferRow<'static>>,
    paths: BTreeMap<Arc<str>, FileId>,
    reads: Arc<AtomicUsize>,
    root: Option<Arc<PathBuf>>,
    complete: bool,
    fail_seal: Arc<AtomicBool>,
    versions: Vec<FileVersion>,
    fail_after_first_batch: bool,
    fail_path_lookup: bool,
}

impl OrderedFiles {
    fn copy_from(source: &dyn FileCatalogSource) -> Self {
        Self::load(source, None, Arc::new(AtomicBool::new(false)), &|| false).unwrap()
    }

    fn load(
        source: &dyn FileCatalogSource,
        root: Option<&Path>,
        fail_seal: Arc<AtomicBool>,
        cancelled: &dyn Fn() -> bool,
    ) -> StorageResult<Self> {
        let mut result = Self {
            allocated: source.high_water_mark(),
            rows: Vec::new(),
            paths: BTreeMap::new(),
            reads: Arc::new(AtomicUsize::new(0)),
            root: root.map(|root| Arc::new(root.to_path_buf())),
            complete: true,
            fail_seal,
            versions: vec![FileVersion::INITIAL; source.high_water_mark() as usize],
            fail_after_first_batch: false,
            fail_path_lookup: false,
        };
        let limit = BatchLimit::new(NonZeroUsize::new(1).unwrap(), NonZeroUsize::MAX);
        transfer_files(source, result.allocated, limit, cancelled, |row| {
            result
                .paths
                .insert(row.entry.relative_path.query_key(), row.entry.id);
            result.rows.push(FileTransferRow {
                entry: Cow::Owned(row.entry.into_owned()),
                fingerprint: row.fingerprint,
                configuration: row.configuration,
            });
            Ok::<_, StorageError>(())
        })?;
        Ok(result)
    }
}

impl FileCatalogSource for OrderedFiles {
    fn high_water_mark(&self) -> u32 {
        self.allocated
    }

    fn live_count(&self) -> u64 {
        self.rows.len() as u64
    }

    fn read_batch<'a>(
        &'a self,
        after: Option<FileId>,
        limit: BatchLimit,
        output: &mut Vec<FileTransferRow<'a>>,
    ) -> StorageResult<BatchStatus> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        if self.fail_after_first_batch && after.is_some() {
            return Err(StorageError::invalid(
                "test file catalog",
                "injected read failure",
            ));
        }
        let position = self
            .rows
            .partition_point(|row| after.is_some_and(|id| row.entry.id.0 <= id.0));
        if let Some(row) = self.rows.get(position) {
            assert!(limit.accepts("test file catalog", 0, 0, row.bytes())?);
            output.push(FileTransferRow {
                entry: Cow::Borrowed(row.entry.as_ref()),
                fingerprint: row.fingerprint,
                configuration: row.configuration,
            });
        }
        Ok(if position + 1 < self.rows.len() {
            BatchStatus::More
        } else {
            BatchStatus::Done
        })
    }
}
impl FileCatalogReader for OrderedFiles {
    fn repository_root(&self) -> Option<&Path> {
        self.root.as_deref().map(PathBuf::as_path)
    }

    fn file(&self, id: FileId) -> StorageResult<Option<Cow<'_, crate::inventory::FileEntry>>> {
        Ok(self
            .rows
            .binary_search_by_key(&id.0, |row| row.entry.id.0)
            .ok()
            .map(|position| Cow::Owned(self.rows[position].entry.as_ref().clone())))
    }

    fn id_for_path(&self, path: &Path) -> StorageResult<Option<FileId>> {
        if self.fail_path_lookup {
            return Err(StorageError::invalid(
                "test file catalog",
                "injected path lookup failure",
            ));
        }
        let key = crate::inventory::SharedPath::from(PathBuf::from(path)).query_key();
        Ok(self.paths.get(&key).copied())
    }

    fn path_candidates(
        &self,
        bounds: FilePathBounds,
        limit: usize,
    ) -> StorageResult<Option<Vec<FileId>>> {
        let mut ids = Vec::new();
        for (path, &id) in &self.paths {
            if bounds.contains(path) {
                if ids.len() == limit {
                    return Ok(None);
                }
                ids.push(id);
            }
        }
        Ok(Some(ids))
    }

    fn visit_paths(
        &self,
        bounds: FilePathBounds,
        visit: &mut dyn FnMut(&[FileId]) -> StorageResult<()>,
    ) -> StorageResult<()> {
        for (path, &id) in &self.paths {
            if bounds.contains(path) {
                visit(&[id])?;
            }
        }
        Ok(())
    }

    fn scalar_candidates(
        &self,
        predicate: FileScalarPredicate<'_>,
        limit: usize,
    ) -> StorageResult<Option<Arc<[FileId]>>> {
        if !matches!(predicate, FileScalarPredicate::UnknownCreated) {
            return Ok(None);
        }
        let mut ids = Vec::new();
        for row in &self.rows {
            if row.entry.created_ns.is_none() {
                if ids.len() == limit {
                    return Ok(None);
                }
                ids.push(row.entry.id);
            }
        }
        Ok(Some(ids.into()))
    }
}

impl OrderedFiles {
    fn position(&self, id: FileId) -> Result<usize, usize> {
        self.rows.binary_search_by_key(&id.0, |row| row.entry.id.0)
    }

    fn write(
        &mut self,
        entry: crate::inventory::FileEntry,
        fingerprint: Option<[u8; 32]>,
        configuration: Option<[u8; 32]>,
        append: bool,
    ) -> StorageResult<()> {
        let path = entry.relative_path.query_key();
        if self.paths.get(&path).is_some_and(|id| *id != entry.id) {
            return Err(StorageError::invalid("test file catalog", "duplicate path"));
        }
        let id = entry.id;
        let row = FileTransferRow {
            entry: Cow::Owned(entry),
            fingerprint,
            configuration,
        };
        if append {
            if id.0 != self.allocated {
                return Err(StorageError::invalid(
                    "test file catalog",
                    "invalid append ID",
                ));
            }
            self.allocated += 1;
            self.versions.push(FileVersion::INITIAL);
            self.rows.push(row);
        } else {
            let position = self.position(id).map_err(|_| {
                StorageError::invalid("test file catalog", "missing replacement ID")
            })?;
            self.paths
                .remove(&self.rows[position].entry.relative_path.query_key());
            self.rows[position] = row;
        }
        self.paths.insert(path, id);
        Ok(())
    }
}

impl FileCatalogRevision for OrderedFiles {
    fn construction_complete(&self) -> bool {
        self.complete
    }

    fn version(&self, id: FileId) -> StorageResult<Option<FileVersion>> {
        Ok(self.versions.get(id.0 as usize).copied())
    }

    fn read_versions(
        &self,
        start: FileId,
        limit: BatchLimit,
    ) -> StorageResult<(Cow<'_, [FileVersion]>, BatchStatus)> {
        let remaining = self.versions.get(start.0 as usize..).ok_or_else(|| {
            StorageError::invalid("test file catalog", "invalid version batch offset")
        })?;
        let batch_len = remaining
            .len()
            .min(limit.fixed_batch_len("test file catalog", size_of::<FileVersion>())?)
            .min(2);
        let end = if remaining.len() == batch_len {
            BatchStatus::Done
        } else {
            BatchStatus::More
        };
        Ok((Cow::Owned(remaining[..batch_len].to_vec()), end))
    }

    fn estimates_delta_bytes(&self) -> usize {
        self.rows.iter().map(FileTransferRow::bytes).sum()
    }

    fn fingerprint(&self, id: FileId) -> StorageResult<Option<[u8; 32]>> {
        Ok(self
            .position(id)
            .ok()
            .and_then(|index| self.rows[index].fingerprint))
    }

    fn configuration(&self, id: FileId) -> StorageResult<Option<[u8; 32]>> {
        Ok(self
            .position(id)
            .ok()
            .and_then(|index| self.rows[index].configuration))
    }

    fn edit(&self) -> StorageResult<Box<dyn FileCatalogEditor>> {
        Ok(Box::new(self.clone()))
    }
}

impl FileCatalogEditor for OrderedFiles {
    fn apply(&mut self, delta: FileCatalogDelta<'_>) -> StorageResult<()> {
        match delta {
            FileCatalogDelta::Append {
                entry,
                fingerprint,
                configuration,
            } => {
                self.write(entry, fingerprint, configuration, true)?;
            }
            FileCatalogDelta::Replace {
                entry,
                fingerprint,
                configuration,
            } => {
                self.write(entry, fingerprint, configuration, false)?;
            }
            FileCatalogDelta::Retire(id) => {
                let position = self.position(id).map_err(|_| {
                    StorageError::invalid("test file catalog", "missing retirement ID")
                })?;
                self.paths
                    .remove(&self.rows.remove(position).entry.relative_path.query_key());
            }
            FileCatalogDelta::AdvanceVersion(id) => {
                let version = self.versions.get_mut(id.0 as usize).ok_or_else(|| {
                    StorageError::invalid("test file catalog", "missing version ID")
                })?;
                *version = version.next().map_err(StorageError::ResourceLimit)?;
            }
            FileCatalogDelta::ResetVersions(version) => self.versions.fill(version),
            FileCatalogDelta::Rebase(base) => {
                // This fixture has bounded size and a contiguous discovery base.
                let next = Self::load(base, base.root(), Arc::clone(&self.fail_seal), &|| false)?;
                if next.allocated != self.allocated {
                    return Err(StorageError::invalid(
                        "test file catalog",
                        "changed rebased ID bound",
                    ));
                }
                for row in &mut self.rows {
                    let position = next.position(row.entry.id).map_err(|_| {
                        StorageError::invalid("test file catalog", "missing rebased row")
                    })?;
                    row.fingerprint = next.rows[position].fingerprint;
                    row.configuration = next.rows[position].configuration;
                }
            }
            FileCatalogDelta::ConstructionComplete(complete) => self.complete = complete,
        }
        Ok(())
    }

    fn seal(self: Box<Self>) -> StorageResult<FileCatalogHandle> {
        if self.fail_seal.swap(false, Ordering::Relaxed) {
            return Err(StorageError::invalid(
                "test file catalog",
                "injected file catalog seal failure",
            ));
        }
        Ok(Arc::from(self as Box<dyn FileCatalogRevision>))
    }
}

#[derive(Default)]
struct OrderedFileFactory {
    fail_seal: Arc<AtomicBool>,
}

impl crate::index::storage::StorageFactory for OrderedFileFactory {
    fn start_file_segment(
        &self,
        input: crate::index::storage::segment::build::FileSegmentBuild<'_>,
    ) -> StorageResult<Box<dyn crate::index::storage::segment::build::SegmentBuilder>> {
        crate::index::storage::default_factory().start_file_segment(input)
    }

    fn seal_direct_segment(
        &self,
        input: crate::index::storage::segment::build::DirectSegmentBuild<'_>,
        seal: crate::index::storage::segment::build::SegmentSeal<'_>,
    ) -> StorageResult<crate::index::storage::segment::build::BuiltSegment> {
        crate::index::storage::default_factory().seal_direct_segment(input, seal)
    }

    fn retain_record_bodies(
        &self,
        bodies: Vec<crate::records::RetainedBody>,
    ) -> StorageResult<crate::index::storage::segment::RetainedBlock> {
        crate::index::storage::default_factory().retain_record_bodies(bodies)
    }

    fn inferred_order(
        &self,
        rows: crate::index::storage::order::InferredOrderRows,
    ) -> StorageResult<Arc<dyn crate::index::storage::order::OrderColumnReader>> {
        crate::index::storage::test_factory().inferred_order(rows)
    }

    fn live_records(
        &self,
        slots: usize,
        initial: crate::index::storage::liveness::InitialLiveness<'_>,
    ) -> StorageResult<crate::index::storage::liveness::ActiveRecords> {
        crate::index::storage::test_factory().live_records(slots, initial)
    }

    fn prepare_file_records(
        &self,
        batch: crate::index::storage::worker::FileWorkerBatch<'_>,
    ) -> StorageResult<crate::construction::batch::BatchContribution> {
        crate::index::storage::default_factory().prepare_file_records(batch)
    }

    fn prepare_direct_records(
        &self,
        batch: crate::index::storage::worker::DirectRecordBatch<'_>,
    ) -> StorageResult<crate::index::storage::worker::PreparedDirectBatch> {
        crate::index::storage::default_factory().prepare_direct_records(batch)
    }

    fn file_catalog(
        &self,
        base: &dyn FileCatalogBase,
        version: FileVersion,
    ) -> StorageResult<FileCatalogHandle> {
        let mut catalog =
            OrderedFiles::load(base, base.root(), Arc::clone(&self.fail_seal), &|| false)?;
        catalog.versions.fill(version);
        Box::new(catalog).seal()
    }

    fn restore_file_catalog(
        &self,
        base: &dyn FileCatalogBase,
        source: &dyn FileCatalogRevision,
        cancelled: &dyn Fn() -> bool,
    ) -> StorageResult<FileCatalogHandle> {
        let mut catalog =
            OrderedFiles::load(source, base.root(), Arc::clone(&self.fail_seal), cancelled)?;
        catalog.versions.clear();
        crate::index::storage::file_catalog::transfer_versions(
            source,
            super::FILE_TRANSFER_LIMIT,
            cancelled,
            |batch| {
                catalog.versions.extend_from_slice(batch);
                Ok(())
            },
        )?;
        Box::new(catalog).seal()
    }

    fn segment_catalog(
        &self,
    ) -> StorageResult<Box<dyn crate::index::storage::segment_catalog::SegmentCatalogEditor>> {
        crate::index::storage::default_factory().segment_catalog()
    }
}

#[test]
fn selected_file_catalog_edits_pin_with_segments_and_abort_on_seal_failure() {
    use crate::lifecycle::Index;
    use crate::repository::configuration::{CollectionDefinition, RepositoryOptions};
    use crate::source::test_support::{fixture_registry, line_format};

    let root = std::env::temp_dir().join(format!(
        "objekte-ordered-file-catalog-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/first.rows"), b"first\nsecond\n").unwrap();
    let factory = Arc::new(OrderedFileFactory::default());
    let index = Index::start_build_with_factory(
        &root,
        vec![CollectionDefinition::folder(
            "entries",
            "data",
            line_format(),
        )],
        RepositoryOptions::default(),
        fixture_registry(),
        Arc::clone(&factory) as Arc<dyn crate::index::storage::StorageFactory>,
    )
    .unwrap()
    .wait_complete()
    .unwrap();
    let pinned = index.view().unwrap();
    let first = Path::new("data/first.rows");
    let second = Path::new("data/second.rows");
    let initial = pinned
        .file_catalog()
        .unwrap()
        .file_by_path(first)
        .unwrap()
        .unwrap();
    assert!(matches!(initial.relative_path, Cow::Owned(_)));
    assert_eq!(
        pinned
            .file_catalog()
            .unwrap()
            .records_for_file(initial.id)
            .unwrap()
            .len(),
        2
    );

    std::fs::write(root.join(second), b"third\nfourth\n").unwrap();
    factory.fail_seal.store(true, Ordering::Relaxed);
    let error = index.add_file(second).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("injected file catalog seal failure")
    );
    let unchanged = index.view().unwrap();
    assert_eq!(unchanged.revision(), pinned.revision());
    assert!(unchanged.file_handle_for_path(second).unwrap().is_none());

    let added = index.add_file(second).unwrap();
    let after_add = index.view().unwrap();
    assert_eq!(
        after_add
            .file_catalog()
            .unwrap()
            .records_for_file(added.file.file_id)
            .unwrap()
            .len(),
        2
    );
    index.delete_file(added.file).unwrap();
    std::fs::remove_dir_all(&root).unwrap();
    index.compact().unwrap();
    assert!(pinned.file_handle_for_path(second).unwrap().is_none());
    assert!(after_add.file_handle_for_path(second).unwrap().is_some());
    let compacted = index.view().unwrap();
    assert_eq!(
        compacted
            .file_version_including_inactive(added.file.file_id)
            .unwrap(),
        Some(added.file.version.next().unwrap())
    );
    assert_eq!(
        after_add.file_version(added.file.file_id).unwrap(),
        Some(added.file.version)
    );
    assert!(compacted.file_handle_for_path(second).unwrap().is_none());
    assert_eq!(
        compacted
            .file_catalog()
            .unwrap()
            .records_for_file(initial.id)
            .unwrap()
            .len(),
        2
    );
    let base = compacted.repository().file_catalog_base();
    let native = crate::index::storage::default_factory()
        .restore_file_catalog(&base, compacted.0.catalog.as_ref(), &|| false)
        .unwrap();
    assert_eq!(
        native.version(added.file.file_id).unwrap(),
        Some(added.file.version.next().unwrap())
    );
    assert!(native.file(added.file.file_id).unwrap().is_none());
    assert_eq!(
        native.version(initial.id).unwrap(),
        pinned.file_version(initial.id).unwrap()
    );
}

use crate::inventory::FileVersion;
use crate::model::Number;
use crate::model::{CollectionId, FileId};
use crate::query::Operator;
use crate::repository::configuration::RepositoryOptions;

#[test]
fn changed_paths_and_tombstones_do_not_copy_or_rewrite_base_inventory() {
    let root = std::env::temp_dir().join(format!(
        "objekte-file-catalog-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/a.row"), b"()").unwrap();
    std::fs::write(root.join("data/b.row"), b"()").unwrap();
    let repository = Arc::new(
        crate::lifecycle::Index::build_repository_for_test(
            &root,
            vec![crate::source::test_support::document_folder("data", "data")],
            RepositoryOptions {
                ..Default::default()
            },
            crate::source::test_support::fixture_registry(),
        )
        .map_err(|error| error.to_string())
        .unwrap(),
    );
    let original = FileCatalog::from_repository(Arc::clone(&repository)).unwrap();
    assert_eq!(original.active_files(), 2);
    assert_eq!(
        original.id_for(std::path::Path::new("data/a.row")),
        Some(FileId(0))
    );

    let mut moved = repository.files().files[0].clone();
    moved.relative_path = "data/renamed.row".into();
    moved.revision.size = 77;
    moved.revision.modified_ns += 100;
    moved.created_ns = Some(777);
    let moved_modified = moved.revision.modified_ns;
    let changed = original
        .replaced(moved, Some([7; 32]), Some([8; 32]))
        .unwrap();
    assert_eq!(
        original.size_ids(Operator::Eq, &Number::integer(2), 8),
        Some(Arc::from([FileId(0), FileId(1)]))
    );
    assert_eq!(
        changed.size_ids(Operator::Eq, &Number::integer(2), 8),
        Some(Arc::from([FileId(1)]))
    );
    assert_eq!(
        changed.size_ids(Operator::Eq, &Number::integer(77), 8),
        Some(Arc::from([FileId(0)]))
    );
    assert_eq!(
        changed.size_ids(Operator::Gt, &Number::integer(2), 8),
        Some(Arc::from([FileId(0)]))
    );
    assert_eq!(
        changed.size_ids(Operator::Lt, &Number::float(2.5).unwrap(), 8),
        Some(Arc::from([FileId(1)]))
    );
    assert_eq!(
        changed.size_ids(Operator::Ge, &Number::float(2.5).unwrap(), 8),
        Some(Arc::from([FileId(0)]))
    );
    assert_eq!(
        changed.size_ids(Operator::Ne, &Number::integer(77), 8),
        Some(Arc::from([FileId(1)]))
    );
    assert_eq!(changed.size_ids(Operator::Ge, &Number::integer(0), 1), None);
    assert_eq!(
        changed.created_ids(Operator::Eq, 777, 8),
        Some(Arc::from([FileId(0)]))
    );
    assert_eq!(
        changed.modified_ids(Operator::Eq, moved_modified, 8),
        Some(Arc::from([FileId(0)]))
    );
    assert_eq!(changed.id_for(std::path::Path::new("data/a.row")), None);
    assert_eq!(
        changed.id_for(std::path::Path::new("data/renamed.row")),
        Some(FileId(0))
    );
    assert_eq!(changed.fingerprint(FileId(0)), Some([7; 32]));
    assert_eq!(
        changed.relative_path_ids(Operator::Eq, "data/renamed.row"),
        Some(vec![FileId(0)])
    );
    assert_eq!(
        changed.relative_path_ids(Operator::Eq, "data/a.row"),
        Some(vec![])
    );
    assert_eq!(changed.configuration(FileId(0)), Some([8; 32]));
    assert_eq!(
        original.id_for(std::path::Path::new("data/a.row")),
        Some(FileId(0))
    );
    assert!(Arc::ptr_eq(&changed.base_files, &original.base_files));

    let retired = changed.retired(FileId(1)).unwrap();
    assert_eq!(retired.active_files(), 1);
    assert_eq!(retired.id_for(std::path::Path::new("data/b.row")), None);
    let mut added = repository.files().files[1].clone();
    added.id = FileId(2);
    added.relative_path = "data/new.row".into();
    added.created_ns = None;
    let appended = retired.appended(added, None, None).unwrap();
    assert!(
        appended
            .unknown_created_ids(8)
            .unwrap()
            .contains(&FileId(2))
    );
    assert_eq!(
        appended.size_ids(Operator::Eq, &Number::integer(77), 8),
        Some(Arc::from([FileId(0)]))
    );
    assert_eq!(appended.allocated_files(), 3);
    assert_eq!(appended.active_files(), 2);
    let mut prefix = appended.relative_path_ids(Operator::Ge, "data/n").unwrap();
    prefix.sort_by_key(|id| id.0);
    assert_eq!(prefix, [FileId(0), FileId(2)]);
    assert_eq!(
        appended.relative_path_ids_up_to(Operator::Ge, "data/n", 1),
        None
    );
    assert_eq!(
        appended.id_for(std::path::Path::new("data/new.row")),
        Some(FileId(2))
    );
    assert_eq!(retired.id_for(std::path::Path::new("data/new.row")), None);
    let scan_baseline = appended.scan_stamps();
    assert_eq!(scan_baseline.len(), 2);
    assert!(scan_baseline.contains_key(std::path::Path::new("data/renamed.row")));
    assert!(scan_baseline.contains_key(std::path::Path::new("data/new.row")));
    assert!(!scan_baseline.contains_key(std::path::Path::new("data/b.row")));

    // Transfer still works after the original files become unavailable.
    std::fs::remove_dir_all(&root).unwrap();
    let mut intermediate = OrderedFiles::copy_from(&appended);
    let mut builder = crate::lifecycle::Index::builder();
    builder
        .collection("entries", crate::schema::Schema::default())
        .unwrap();
    let empty_view = builder.build().unwrap().view().unwrap();
    let path = Path::new("data/new.row");
    {
        let service = crate::files::FileCatalogView {
            view: &empty_view,
            reader: &intermediate,
        };
        let row = service.file_by_path(path).unwrap().unwrap();
        assert!(matches!(row.relative_path, Cow::Owned(_)));
        assert_eq!(row.relative_path.as_ref(), path);
        assert_eq!(row.id, FileId(2));
        assert!(
            service
                .file_by_path(Path::new("data/b.row"))
                .unwrap()
                .is_none()
        );
        assert!(
            service
                .files_without_creation_time()
                .unwrap()
                .iter()
                .any(|row| row.id == FileId(2))
        );
    }
    intermediate.fail_path_lookup = true;
    let service = crate::files::FileCatalogView {
        view: &empty_view,
        reader: &intermediate,
    };
    assert!(matches!(
        service.file_by_path(path),
        Err(crate::files::LookupError::Query(message)) if message.contains("injected path lookup failure")
    ));
    intermediate.fail_path_lookup = false;
    intermediate.allocated += 1; // preserve a trailing retired file ID
    let restored = FileCatalog::restore_from(&repository, &intermediate, &|| false).unwrap();
    assert_eq!(intermediate.reads.load(Ordering::Relaxed), 2);
    assert_eq!(restored.allocated_files(), 4);
    assert_eq!(restored.active_files(), 2);
    assert!(Arc::ptr_eq(&restored.base_files, &repository.files));
    assert_eq!(restored.id_for(std::path::Path::new("data/a.row")), None);
    assert_eq!(restored.id_for(std::path::Path::new("data/b.row")), None);
    assert_eq!(
        restored.id_for(std::path::Path::new("data/renamed.row")),
        Some(FileId(0))
    );
    assert_eq!(
        restored.id_for(std::path::Path::new("data/new.row")),
        Some(FileId(2))
    );
    assert_eq!(restored.fingerprint(FileId(0)), Some([7; 32]));
    assert_eq!(
        restored.relative_path_ids(Operator::Eq, "data/new.row"),
        Some(vec![FileId(2)])
    );
    assert_eq!(restored.configuration(FileId(0)), Some([8; 32]));
    assert!(restored.entry(FileId(1)).is_none());

    assert!(restored.entry(FileId(3)).is_none());
    let baseline = FileCatalog::restore_from(&repository, &original, &|| false).unwrap();
    assert!(baseline.changed.iter().all(|bucket| bucket.is_empty()));
    assert!(
        baseline
            .changed_paths
            .iter()
            .all(|bucket| bucket.is_empty())
    );

    intermediate.fail_after_first_batch = true;
    assert!(FileCatalog::restore_from(&repository, &intermediate, &|| false).is_err());
    intermediate.fail_after_first_batch = false;
    assert_eq!(
        FileCatalog::restore_from(&repository, &intermediate, &|| true).err(),
        Some(StorageError::Cancelled)
    );
    assert!(FileCatalog::restore_from(&repository, &intermediate, &|| false).is_ok());

    let original_entry = intermediate.rows[1].entry.clone();
    intermediate.rows[1].entry.to_mut().relative_path = "data/renamed.row".into();
    assert!(FileCatalog::restore_from(&repository, &intermediate, &|| false).is_err());
    intermediate.rows[1].entry = original_entry.clone();
    intermediate.rows[1].entry.to_mut().id = FileId(7);
    assert!(FileCatalog::restore_from(&repository, &intermediate, &|| false).is_err());
    intermediate.rows[1].entry = original_entry;
    intermediate.rows[1].entry.to_mut().collections = vec![CollectionId(9)];
    assert!(FileCatalog::restore_from(&repository, &intermediate, &|| false).is_err());
    intermediate.allocated = 1;
    assert!(FileCatalog::restore_from(&repository, &intermediate, &|| false).is_err());
}

#[test]
fn scalar_cap_counts_live_members_after_base_and_delta_churn() {
    use crate::inventory::{FileInventory, SourceRevision};
    use std::path::PathBuf;

    const FILES: u32 = 4097;
    let revision = SourceRevision {
        size: 73,
        modified_ns: 100,
        object_id: None,
    };
    let collection = CollectionId(0);
    let root = PathBuf::from("catalog");
    for base in [false, true] {
        let mut inventory = FileInventory::new(root.clone());
        if base {
            for number in 0..FILES {
                inventory
                    .add(format!("{number}.row").into(), revision, None, collection)
                    .unwrap();
            }
        }
        let mut catalog = FileCatalog::from_parts(
            Arc::new(inventory),
            Arc::new(Vec::new()),
            [0; 32],
            FileVersion::INITIAL,
        );
        let entry = |number| crate::inventory::FileEntry {
            id: FileId(number),
            relative_path: format!("{number}.row").into(),
            revision,
            created_ns: None,
            collections: vec![collection],
            status: crate::inventory::FileStatus::Valid,
            diagnostics: Vec::new(),
            omitted_diagnostics: 0,
        };
        if !base {
            for number in 0..FILES {
                catalog = catalog.appended(entry(number), None, None).unwrap();
            }
        }
        let pinned = catalog.clone();
        let equal =
            |catalog: &FileCatalog| catalog.size_ids(Operator::Eq, &Number::integer(73), 4096);
        assert!(equal(&pinned).is_none());
        let bounds = (std::ops::Bound::Unbounded, std::ops::Bound::Unbounded);
        assert!(
            catalog
                .path_candidates(bounds.clone(), 256)
                .unwrap()
                .is_none()
        );
        let mut count = 0usize;
        catalog
            .visit_paths(bounds.clone(), &mut |batch| {
                assert!(!batch.is_empty() && batch.len() <= 256);
                count += batch.len();
                Ok(())
            })
            .unwrap();
        assert_eq!(count, FILES as usize);
        let mut batches = 0;
        let result = catalog.visit_paths(bounds, &mut |_| {
            batches += 1;
            Err(StorageError::Cancelled)
        });
        assert_eq!(result, Err(StorageError::Cancelled));
        assert_eq!(batches, 1);
        for number in 0..FILES - 1 {
            catalog = catalog.retired(FileId(number)).unwrap();
        }
        assert_eq!(
            equal(&catalog).as_deref(),
            Some([FileId(FILES - 1)].as_slice())
        );
        assert_eq!(
            catalog.unknown_created_ids(4096).as_deref(),
            Some([FileId(FILES - 1)].as_slice())
        );
        assert_eq!(
            catalog.modified_ids(Operator::Eq, 100, 4096).as_deref(),
            Some([FileId(FILES - 1)].as_slice())
        );
        catalog = catalog.retired(FileId(FILES - 1)).unwrap();
        catalog = catalog.appended(entry(FILES), None, None).unwrap();
        let ids = equal(&catalog).unwrap();
        assert_eq!(&*ids, &[FileId(FILES)]);
        assert!(Arc::ptr_eq(&ids, &equal(&catalog).unwrap()));
        assert!(equal(&pinned).is_none());
    }
}
