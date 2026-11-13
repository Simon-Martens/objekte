use objekte_search_core::lifecycle::Index;
use objekte_search_core::repository::{
    CollectionDefinition, Repository, RepositoryOptions, SnapshotMode, SnapshotOpenPolicy,
};
use objekte_search_core::source::SourceFormat;

#[test]
fn production_open_and_save_cannot_use_deprecated_snapshots() {
    let root = std::env::temp_dir().join(format!(
        "objekte-legacy-disabled-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/a.json"), br#"{"title":"A"}"#).unwrap();
    let definition = CollectionDefinition::folder("items", "data", SourceFormat::Json);
    let old_path = root.join("search-core-ascii.snapshot");
    let repo = Repository::open(&root, vec![definition.clone()]).unwrap();
    assert!(!repo.startup().restored);
    assert!(repo.save_snapshot(&old_path).is_err());
    assert!(!old_path.exists());
    assert!(
        Repository::open_with_snapshot_policy(
            &root,
            vec![definition.clone()],
            RepositoryOptions::default(),
            SnapshotOpenPolicy::new(SnapshotMode::IfValid, None),
        )
        .is_err()
    );
    assert!(
        Index::load_cache(
            &root,
            vec![definition.clone()],
            RepositoryOptions::default(),
            &old_path,
        )
        .is_err()
    );
    let index = Index::build(&root, vec![definition], RepositoryOptions::default()).unwrap();
    assert!(index.flush_cache(&old_path).is_err());
    assert!(!old_path.exists());
    std::fs::remove_dir_all(root).unwrap();
}
