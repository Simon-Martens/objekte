use std::sync::Arc;

use super::FileCatalog;
use crate::model::FileId;
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
    assert_eq!(
        appended.id_for(std::path::Path::new("data/new.json")),
        Some(FileId(2))
    );
    assert_eq!(retired.id_for(std::path::Path::new("data/new.json")), None);
    std::fs::remove_dir_all(root).unwrap();
}
