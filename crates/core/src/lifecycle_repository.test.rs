use super::*;
use crate::query::{Query, QueryOptions};
use crate::workload::ReturnMode;

fn fixture(label: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let root = std::env::temp_dir().join(format!(
        "objekte-convenience-{label}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    fs::create_dir_all(root.join(".objekte/collections")).unwrap();
    fs::create_dir_all(root.join("data")).unwrap();
    fs::write(
        root.join(".objekte/collections/data.schema.json"),
        br#"{
            "type":"object",
            "properties":{"name":{"type":"string","x-index":true}},
            "x-objekte":{"name":"data","path":"data","format":"json","id":"filename"}
        }"#,
    )
    .unwrap();
    fs::write(root.join("data/a.json"), br#"{"name":"A"}"#).unwrap();
    root
}

fn count(index: &Index) -> usize {
    let query = Query {
        collections: vec!["data".into()],
        fts: None,
        filter: None,
        options: QueryOptions {
            sort: Vec::new(),
            limit: 10,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    index.view().unwrap().search(&query).unwrap().count.unwrap()
}

#[test]
fn default_configured_open_ignores_legacy_cache_without_touching_it() {
    let root = fixture("memory-default");
    let cache = snapshot_path_in(
        &root.join(".objekte"),
        RepositoryOptions::default().bigram_alphabet,
    );
    fs::write(&cache, b"not an index").unwrap();
    let index = Index::open_repository(&root, RepositoryOpenOptions::default()).unwrap();
    assert_eq!(count(&index), 1);
    assert!(!index.restored_from_cache());
    assert_eq!(fs::read(&cache).unwrap(), b"not an index");
    assert!(index.status().unwrap().dirty());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn convenience_build_and_restore_use_one_resolved_cache_without_automatic_writes() {
    let root = fixture("build-restore");
    let options = RepositoryOpenOptions {
        cache_directory: Some(PathBuf::from("data/cache")),
        cache_open: CacheOpenPolicy::IfPresent,
        ..Default::default()
    };
    let cache = snapshot_path_in(&root.join("data/cache"), options.index.bigram_alphabet);
    let built = Index::open_repository(&root, options.clone()).unwrap();
    assert_eq!(count(&built), 1);
    assert!(built.status().unwrap().dirty());
    assert!(!cache.exists());
    built.flush_cache(&cache).unwrap();
    assert!(!built.status().unwrap().dirty());

    let restored = Index::open_repository(&root, options).unwrap();
    assert_eq!(count(&restored), 1);
    assert!(!restored.status().unwrap().dirty());
    let report = restored.wait_for_reconciliation().unwrap().unwrap();
    assert_eq!(report.unchanged, 1);
    assert!(report.issues.is_empty(), "{:?}", report.issues);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn repository_cache_writer_can_be_enabled_after_opening() {
    let root = fixture("runtime-sync-toggle");
    let cache = snapshot_path_in(
        &root.join(".objekte"),
        RepositoryOptions::default().bigram_alphabet,
    );
    let index = Index::open_repository(
        &root,
        RepositoryOpenOptions {
            cache_open: CacheOpenPolicy::IfPresent,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(!cache.exists());
    index.set_sync_cache(true).unwrap();
    index.set_sync_cache(false).unwrap();
    assert!(cache.exists());
    assert!(!index.status().unwrap().dirty());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn corrupt_convenience_cache_is_replaced_under_explicit_rebuild_policy() {
    let root = fixture("corrupt");
    let cache = snapshot_path_in(
        &root.join(".objekte"),
        RepositoryOptions::default().bigram_alphabet,
    );
    fs::write(&cache, b"corrupt cache").unwrap();
    let explicit = RepositoryOpenOptions {
        cache_open: CacheOpenPolicy::IfPresent,
        ..Default::default()
    };
    let rebuilt = Index::open_repository(&root, explicit.clone()).unwrap();
    assert_eq!(count(&rebuilt), 1);
    assert!(!rebuilt.status().unwrap().dirty());
    assert!(!rebuilt.restored_from_cache());
    assert_ne!(fs::read(&cache).unwrap(), b"corrupt cache");
    let restored = Index::open_repository(&root, explicit).unwrap();
    assert!(restored.restored_from_cache());
    restored.wait_for_reconciliation().unwrap();
    fs::write(&cache, b"corrupt cache").unwrap();
    assert!(matches!(
        Index::open_repository(
            &root,
            RepositoryOpenOptions {
                cache_failure: CacheFailurePolicy::Report,
                cache_open: CacheOpenPolicy::IfPresent,
                ..Default::default()
            }
        ),
        Err(IndexError::InvalidCache(_))
    ));
    assert_eq!(fs::read(&cache).unwrap(), b"corrupt cache");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn invalid_cache_aliasing_a_schema_is_not_deleted() {
    let root = fixture("protected-corrupt");
    let schema = root.join(".objekte/collections/data.schema.json");
    let original = fs::read(&schema).unwrap();
    let cache = snapshot_path_in(
        &root.join(".objekte"),
        RepositoryOptions::default().bigram_alphabet,
    );
    fs::hard_link(&schema, &cache).unwrap();
    let error = Index::open_repository(&root, RepositoryOpenOptions {
        cache_open: CacheOpenPolicy::IfPresent,
        ..Default::default()
    })
        .err()
        .unwrap();
    assert!(matches!(error, IndexError::InvalidCache(_)), "{error}");
    assert_eq!(fs::read(&schema).unwrap(), original);
    assert_eq!(fs::read(&cache).unwrap(), original);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn sync_cache_persists_initial_build_at_selected_destination() {
    let root = fixture("sync");
    let options = RepositoryOpenOptions {
        sync_cache: true,
        ..Default::default()
    };
    let cache = snapshot_path_in(&root.join(".objekte"), options.index.bigram_alphabet);
    let index = Index::open_repository(&root, options).unwrap();
    assert_eq!(count(&index), 1);
    index.stop_sync_cache().unwrap();
    assert!(cache.is_file());
    assert!(!index.status().unwrap().dirty());
    assert!(index.sync_cache_error().unwrap().is_none());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn explicit_convenience_open_policy_can_require_or_ignore_cache() {
    let root = fixture("policy");
    assert!(matches!(
        Index::open_repository(
            &root,
            RepositoryOpenOptions {
                cache_open: CacheOpenPolicy::Required,
                ..Default::default()
            },
        ),
        Err(IndexError::InvalidCache(_))
    ));
    let cache = snapshot_path_in(
        &root.join(".objekte"),
        RepositoryOptions::default().bigram_alphabet,
    );
    fs::write(&cache, b"corrupt cache").unwrap();
    let built = Index::open_repository(
        &root,
        RepositoryOpenOptions {
            cache_open: CacheOpenPolicy::Ignore,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(count(&built), 1);
    assert_eq!(fs::read(cache).unwrap(), b"corrupt cache");
    fs::remove_dir_all(root).unwrap();
}
