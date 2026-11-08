use super::*;
use crate::query::{Query, QueryOptions};
use crate::source::SourceFormat;
use crate::workload::ReturnMode;

#[test]
fn explicit_build_ignores_implicit_cache_even_when_legacy_mode_requires_it() {
    let root = std::env::temp_dir().join(format!(
        "objekte-explicit-build-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::create_dir_all(root.join(".objekte")).unwrap();
    std::fs::write(root.join("data/a.json"), b"{\"name\":\"A\"}").unwrap();
    let cache = root.join(".objekte/search-core-ascii.snapshot");
    std::fs::write(&cache, b"invalid cache contents").unwrap();
    let index = Index::build(
        &root,
        vec![CollectionDefinition::folder(
            "data",
            "data",
            SourceFormat::Json,
        )],
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Required,
            ..Default::default()
        },
    )
    .unwrap();
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
    assert_eq!(index.view().unwrap().search(&query).unwrap().count, Some(1));
    assert_eq!(std::fs::read(&cache).unwrap(), b"invalid cache contents");
    assert!(index.status().unwrap().dirty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn lifecycle_cache_restores_segments_and_excludes_its_own_source_path() {
    let root = std::env::temp_dir().join(format!(
        "objekte-explicit-lifecycle-cache-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    let a = root.join("data/a.jsonl");
    std::fs::write(&a, b"{\"name\":\"old\"}\n").unwrap();
    let definitions = vec![CollectionDefinition::folder(
        "data",
        "data",
        SourceFormat::JsonLines,
    )];
    let index = Index::build(&root, definitions.clone(), RepositoryOptions::default()).unwrap();
    let b = root.join("data/b.jsonl");
    std::fs::write(&b, b"{\"name\":\"b\"}\n").unwrap();
    let prepared = index.prepare_add_file(&b).unwrap();
    let expected = prepared.expected_revision;
    index
        .publish_prepared(expected, index.stage_prepared_add(prepared).unwrap())
        .unwrap();
    let handle = index.view().unwrap().file_handle_for_path(&a).unwrap();
    std::fs::write(&a, b"{\"name\":\"new\"}\n").unwrap();
    let prepared = index.prepare_replace_file(handle, &a).unwrap();
    let expected = prepared.expected_revision;
    index
        .publish_prepared(expected, index.stage_prepared_replace(prepared).unwrap())
        .unwrap();
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
    let before = index.view().unwrap().search(&query).unwrap();
    assert_eq!(before.count, Some(2));
    let cache = root.join("data/cache.jsonl");
    let saved = index.flush_cache(&cache).unwrap();
    assert_eq!(saved, index.status().unwrap().revision);
    assert!(!index.status().unwrap().dirty());
    assert!(std::fs::metadata(&cache).unwrap().len() > 0);
    assert!(matches!(
        index.flush_cache(&a),
        Err(IndexError::InvalidCache(_))
    ));
    assert_eq!(std::fs::read(&a).unwrap(), b"{\"name\":\"new\"}\n");

    let restored =
        Index::load_cache(&root, definitions, RepositoryOptions::default(), &cache).unwrap();
    assert_eq!(restored.status().unwrap().revision, saved);
    assert_eq!(
        restored.view().unwrap().search(&query).unwrap().hits,
        before.hits
    );
    let reconciliation = restored.wait_for_reconciliation().unwrap().unwrap();
    assert_eq!(reconciliation.unchanged, 2);
    assert_eq!(
        (
            reconciliation.added,
            reconciliation.updated,
            reconciliation.removed
        ),
        (0, 0, 0)
    );
    assert!(
        reconciliation.issues.is_empty(),
        "{:?}",
        reconciliation.issues
    );
    assert_eq!(restored.status().unwrap().indexed_files, 2);
    assert!(!restored.status().unwrap().dirty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn automatic_cache_writer_coalesces_mutations_and_saves_latest_revision() {
    let root = std::env::temp_dir().join(format!(
        "objekte-sync-cache-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/a.jsonl"), b"{\"name\":\"a\"}\n").unwrap();
    let definitions = vec![CollectionDefinition::folder(
        "data",
        "data",
        SourceFormat::JsonLines,
    )];
    let index = Index::build(&root, definitions.clone(), RepositoryOptions::default()).unwrap();
    let cache = root.join("cache.snapshot");
    index
        .select_cache_destination(&cache, None, Vec::new())
        .unwrap();
    index.enable_sync_cache().unwrap();
    for name in ["b", "c"] {
        let path = root.join(format!("data/{name}.jsonl"));
        std::fs::write(&path, format!("{{\"name\":\"{name}\"}}\n")).unwrap();
        let prepared = index.prepare_add_file(&path).unwrap();
        let expected = prepared.expected_revision;
        index
            .publish_prepared(expected, index.stage_prepared_add(prepared).unwrap())
            .unwrap();
    }
    index.stop_sync_cache().unwrap();
    assert_eq!(index.sync_cache_error().unwrap(), None);
    assert_eq!(
        index.status().unwrap().saved_revision,
        Some(index.view().unwrap().revision())
    );
    let restored =
        Index::load_cache(&root, definitions, RepositoryOptions::default(), &cache).unwrap();
    assert_eq!(
        restored
            .view()
            .unwrap()
            .search(&Query {
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
            })
            .unwrap()
            .count,
        Some(3)
    );
    restored.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
