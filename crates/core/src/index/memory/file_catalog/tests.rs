use std::borrow::Cow;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::FileCatalog;
use crate::index::storage::file_catalog::{
    FileCatalogReader, FileCatalogSource, FileTransferRow, transfer_files,
};
use crate::index::storage::transfer::{BatchLimit, BatchStatus, StorageError, StorageResult};

/// An independent catalog used in transfer tests. Store only ordered live
/// rows, with no native inventory, index, masks, or lookup tables.
struct OrderedFiles {
    allocated: u32,
    rows: Vec<FileTransferRow<'static>>,
    reads: AtomicUsize,
    fail_after_first_batch: bool,
}

impl OrderedFiles {
    fn copy_from(source: &dyn FileCatalogSource) -> Self {
        let mut result = Self {
            allocated: source.high_water_mark(),
            rows: Vec::new(),
            reads: AtomicUsize::new(0),
            fail_after_first_batch: false,
        };
        let limit = BatchLimit::new(NonZeroUsize::new(1).unwrap(), NonZeroUsize::MAX);
        transfer_files(source, result.allocated, limit, &|| false, |row| {
            result.rows.push(FileTransferRow {
                entry: Cow::Owned(row.entry.into_owned()),
                fingerprint: row.fingerprint,
                configuration: row.configuration,
            });
            Ok::<_, StorageError>(())
        })
        .unwrap();
        result
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
        let mut catalog = FileCatalog::from_parts(Arc::new(inventory), Vec::new(), [0; 32]);
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
