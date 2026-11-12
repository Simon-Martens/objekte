use super::*;
use crate::query::{Query, QueryOptions};
use crate::source::SourceFormat;
use crate::workload::ReturnMode;

#[test]
fn staged_build_matches_static_inference_overlap_and_cache_restore() {
    use crate::inference::{InferenceConfig, InferenceMode};

    let root = std::env::temp_dir().join(format!(
        "objekte-direct-json-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/a.json"), br#"{"name":"alpha","n":1}"#).unwrap();
    std::fs::write(root.join("data/b.json"), br#"{"name":"beta","n":2}"#).unwrap();
    std::fs::write(root.join("data/c.json"), b"{broken").unwrap();
    let definitions = ["one", "two"]
        .into_iter()
        .map(|name| CollectionDefinition::folder(name, "data", SourceFormat::Json))
        .collect::<Vec<_>>();
    let mut inference = InferenceConfig {
        mode: InferenceMode::AutoIndex,
        ..Default::default()
    };
    inference.policy.min_nonblank_occurrences = 1;
    inference.policy.min_nonblank_records = 1;
    let options = RepositoryOptions {
        inference,
        ..Default::default()
    };
    let static_repo = Repository::open_with_options(&root, definitions.clone(), options).unwrap();
    let index = Index::build(&root, definitions.clone(), options).unwrap();
    assert_eq!(index.status().unwrap().indexed_files, 3);
    assert_eq!(index.status().unwrap().live_records, 6);
    assert!(
        index
            .view()
            .unwrap()
            .repository()
            .collections()
            .iter()
            .all(|state| { state.generation.records.is_empty() })
    );
    assert_eq!(
        index.view().unwrap().repository().build_parses(),
        static_repo.build_parses() * 2
    );
    let query = Query {
        collections: vec!["one".into(), "two".into()],
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
    let expected = static_repo.search(&query).unwrap();
    let check = |index: &Index| {
        let view = index.view().unwrap();
        let actual = view.search(&query).unwrap();
        assert_eq!(actual.count, expected.count);
        let hits = |result: crate::repository::SearchResult| {
            let mut hits = result
                .hits
                .into_iter()
                .map(|hit| {
                    (
                        hit.collection,
                        hit.record_id,
                        hit.file_path,
                        hit.file_status,
                    )
                })
                .collect::<Vec<_>>();
            hits.sort_by(|left, right| left.0.cmp(&right.0).then(left.1.cmp(&right.1)));
            hits
        };
        assert_eq!(hits(actual), hits(expected.clone()));
        for name in ["one", "two"] {
            assert_eq!(
                serde_json::to_value(view.repository().inference_report(name).unwrap()).unwrap(),
                serde_json::to_value(static_repo.inference_report(name).unwrap()).unwrap()
            );
            assert_eq!(
                serde_json::to_value(view.repository().inference_index_diagnostics(name).unwrap())
                    .unwrap(),
                serde_json::to_value(static_repo.inference_index_diagnostics(name).unwrap())
                    .unwrap()
            );
        }
    };
    check(&index);
    let cache = root.join("direct.snapshot");
    index.flush_cache(&cache).unwrap();
    let restored = Index::load_cache(&root, definitions, options, &cache).unwrap();
    check(&restored);
    restored.shutdown().unwrap();
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

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
fn explicit_schema_path_is_protected_from_cache_flush() {
    let root = std::env::temp_dir().join(format!(
        "objekte-explicit-schema-guard-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/a.json"), b"{}").unwrap();
    let schema = root.join("selected.schema.json");
    std::fs::write(&schema, b"{\"title\":\"schema\"}").unwrap();
    let index = Index::build(
        &root,
        vec![CollectionDefinition::folder(
            "data",
            "data",
            SourceFormat::Json,
        )],
        RepositoryOptions::default(),
    )
    .unwrap();
    index
        .protect_configuration_files(&[schema.clone()])
        .unwrap();
    assert!(matches!(
        index.flush_cache(&schema),
        Err(IndexError::InvalidCache(_))
    ));
    assert_eq!(std::fs::read(&schema).unwrap(), b"{\"title\":\"schema\"}");
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
    let observed = restored.current_reconciliation_task().unwrap().unwrap();
    assert_eq!(restored.status().unwrap().revision, saved);
    assert_eq!(
        restored.view().unwrap().search(&query).unwrap().hits,
        before.hits
    );
    let reconciliation = restored.wait_for_reconciliation().unwrap().unwrap();
    assert_eq!(observed.wait().unwrap(), reconciliation);
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
