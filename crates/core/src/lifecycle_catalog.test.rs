use std::sync::Arc;

use super::FileCatalog;
use crate::model::{CollectionId, FileId};
use crate::query::Operator;
use crate::repository::{CollectionDefinition, Repository, RepositoryOptions, SnapshotMode};
use crate::source::SourceFormat;

#[test]
fn changed_paths_and_tombstones_do_not_copy_or_rewrite_base_inventory() {
    let root = std::env::temp_dir().join(format!(
        "objekte-file-catalog-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/a.json"), b"{}").unwrap();
    std::fs::write(root.join("data/b.json"), b"{}").unwrap();
    let repository = Arc::new(
        Repository::open_with_options(
            &root,
            vec![CollectionDefinition::folder(
                "data",
                "data",
                SourceFormat::Json,
            )],
            RepositoryOptions {
                snapshot_mode: SnapshotMode::Disabled,
                ..Default::default()
            },
        )
        .unwrap(),
    );
    let original = FileCatalog::from_repository(Arc::clone(&repository)).unwrap();
    assert_eq!(original.active_files(), 2);
    assert_eq!(
        original.id_for(std::path::Path::new("data/a.json")),
        Some(FileId(0))
    );

    let mut moved = repository.files().files[0].clone();
    moved.relative_path = "data/renamed.json".into();
    let changed = original
        .replaced(moved, Some([7; 32]), Some([8; 32]))
        .unwrap();
    assert_eq!(changed.id_for(std::path::Path::new("data/a.json")), None);
    assert_eq!(
        changed.id_for(std::path::Path::new("data/renamed.json")),
        Some(FileId(0))
    );
    assert_eq!(changed.fingerprint(FileId(0)), Some([7; 32]));
    assert_eq!(
        changed.relative_path_ids(Operator::Eq, "data/renamed.json"),
        Some(vec![FileId(0)])
    );
    assert_eq!(
        changed.relative_path_ids(Operator::Eq, "data/a.json"),
        Some(vec![])
    );
    assert_eq!(changed.configuration(FileId(0)), Some([8; 32]));
    assert_eq!(
        original.id_for(std::path::Path::new("data/a.json")),
        Some(FileId(0))
    );
    assert!(Arc::ptr_eq(&changed.base, &original.base));

    let retired = changed.retired(FileId(1)).unwrap();
    assert_eq!(retired.active_files(), 1);
    assert_eq!(retired.id_for(std::path::Path::new("data/b.json")), None);
    let mut added = repository.files().files[1].clone();
    added.id = FileId(2);
    added.relative_path = "data/new.json".into();
    let appended = retired.appended(added, None, None).unwrap();
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
        appended.id_for(std::path::Path::new("data/new.json")),
        Some(FileId(2))
    );
    assert_eq!(retired.id_for(std::path::Path::new("data/new.json")), None);
    let scan_baseline = appended.scan_stamps();
    assert_eq!(scan_baseline.len(), 2);
    assert!(scan_baseline.contains_key(std::path::Path::new("data/renamed.json")));
    assert!(scan_baseline.contains_key(std::path::Path::new("data/new.json")));
    assert!(!scan_baseline.contains_key(std::path::Path::new("data/b.json")));

    let slots = (0..appended.allocated_files())
        .map(|number| {
            let id = FileId(number as u32);
            appended
                .entry(id)
                .cloned()
                .map(|entry| (entry, appended.fingerprint(id), appended.configuration(id)))
        })
        .collect::<Vec<_>>();
    let restored = FileCatalog::restored(Arc::clone(&repository), slots.clone()).unwrap();
    assert_eq!(restored.allocated_files(), 3);
    assert_eq!(restored.active_files(), 2);
    assert!(Arc::ptr_eq(&restored.base, &repository));
    assert_eq!(restored.id_for(std::path::Path::new("data/a.json")), None);
    assert_eq!(restored.id_for(std::path::Path::new("data/b.json")), None);
    assert_eq!(
        restored.id_for(std::path::Path::new("data/renamed.json")),
        Some(FileId(0))
    );
    assert_eq!(
        restored.id_for(std::path::Path::new("data/new.json")),
        Some(FileId(2))
    );
    assert_eq!(restored.fingerprint(FileId(0)), Some([7; 32]));
    assert_eq!(
        restored.relative_path_ids(Operator::Eq, "data/new.json"),
        Some(vec![FileId(2)])
    );
    assert_eq!(restored.configuration(FileId(0)), Some([8; 32]));
    assert!(restored.entry(FileId(1)).is_none());

    let unchanged = original.snapshot_slots();
    let baseline = FileCatalog::restored(Arc::clone(&repository), unchanged).unwrap();
    assert!(baseline.changed.iter().all(|bucket| bucket.is_empty()));
    assert!(
        baseline
            .changed_paths
            .iter()
            .all(|bucket| bucket.is_empty())
    );

    let mut duplicate_path = slots.clone();
    duplicate_path[2].as_mut().unwrap().0.relative_path = "data/renamed.json".into();
    assert!(FileCatalog::restored(Arc::clone(&repository), duplicate_path).is_err());
    let mut wrong_id = slots.clone();
    wrong_id[2].as_mut().unwrap().0.id = FileId(7);
    assert!(FileCatalog::restored(Arc::clone(&repository), wrong_id).is_err());
    let mut invalid_membership = slots;
    invalid_membership[2].as_mut().unwrap().0.collections = vec![CollectionId(9)];
    assert!(FileCatalog::restored(Arc::clone(&repository), invalid_membership).is_err());
    assert!(FileCatalog::restored(Arc::clone(&repository), vec![None]).is_err());
    std::fs::remove_dir_all(root).unwrap();
}
