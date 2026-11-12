use super::*;
use crate::query::{Query, QueryOptions};
use crate::source::SourceFormat;
use crate::workload::ReturnMode;

#[test]
fn extended_paused_before_search_ready_resumes() {
    let root = std::env::temp_dir().join(format!(
        "objekte-pre-ready-pause-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/a.json"), br#"{"name":"Ada"}"#).unwrap();
    let mut definition = CollectionDefinition::folder("data", "data", SourceFormat::Json);
    definition
        .schema
        .register_field(crate::reference::FieldDefinition {
            path: vec!["name".into()],
            kind: crate::model::LogicalType::String,
            required: false,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    let task = Index::start_staged_build_with_sources_and_cache(
        &root,
        vec![definition],
        RepositoryOptions::default(),
        SourceRegistry::builtin(),
        None,
        true,
    )
    .unwrap();
    let index = task.wait_search_ready().unwrap();
    assert_eq!(
        index.extended_status().unwrap().phase,
        crate::lifecycle::ExtendedPhase::Paused
    );
    task.resume_extended().unwrap();
    index.wait_extended().unwrap();
    assert_eq!(
        index.extended_status().unwrap().phase,
        crate::lifecycle::ExtendedPhase::Complete
    );
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn explicit_inference_covers_nested_and_multivalued_states_exactly() {
    let root = std::env::temp_dir().join(format!(
        "objekte-inferred-states-{}-{:?}",
        std::process::id(), std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/a.json"),
        br#"{"outer":{"inner":"x"},"items":[{"title":null},{"title":"x"}]}"#).unwrap();
    std::fs::write(root.join("data/b.json"), br#"{"outer":{},"items":[]}"#).unwrap();
    let definition = CollectionDefinition::folder("data", "data", SourceFormat::Json);
    let mut inference = crate::inference::InferenceConfig {
        mode: crate::inference::InferenceMode::AutoIndex,
        ..Default::default()
    };
    inference.policy.min_nonblank_occurrences = 1;
    inference.policy.min_nonblank_records = 1;
    let options = RepositoryOptions { inference, ..Default::default() };
    let index = Index::start_staged_build(&root, vec![definition], options)
        .unwrap().wait_complete().unwrap();
    let view = index.view().unwrap();
    let batches = view.extended_batches(crate::model::CollectionId(0)).unwrap();
    assert!(batches.iter().any(|batch| batch.field_states.contains_key(&vec!["outer".into(), "inner".into()])));
    assert!(batches.iter().any(|batch| batch.field_states.contains_key(&vec!["items".into(), "title".into()])));
    assert!(batches.iter().flat_map(|batch| batch.field_states.keys()).all(|path| !path.iter().any(|part| part.parse::<usize>().is_ok())));
    for (filter, expected) in [
        ("items.title:null = true", 1),
        ("items.title:null = false", 1),
        ("outer.inner:string = true", 1),
    ] {
        let request = Query {
            collections: vec!["data".into()], fts: None,
            filter: Some(crate::query::parse_filter(filter).unwrap()),
            options: QueryOptions {
                sort: vec![], limit: 10, offset: 0, count: true,
                return_mode: ReturnMode::Locator, evidence: false,
            },
        };
        assert_eq!(view.search(&request).unwrap().count, Some(expected), "{filter}");
        assert_eq!(view.search(&request).unwrap().count, view.search_reference(&request).unwrap().count, "{filter}");
    }
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn later_inference_reuses_searchable_base_and_updates_cache_policy() {
    let root = std::env::temp_dir().join(format!(
        "objekte-later-inference-{}-{:?}",
        std::process::id(), std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/a.json"), br#"{"nested":{"title":"Ada"}}"#).unwrap();
    let definitions = vec![CollectionDefinition::folder("data", "data", SourceFormat::Json)];
    let index = Index::start_staged_build(&root, definitions.clone(), RepositoryOptions::default())
        .unwrap().wait_complete().unwrap();
    let base_cache = root.join("base.snapshot");
    index.flush_cache(&base_cache).unwrap();
    let active = Index::load_cache(&root, definitions.clone(), RepositoryOptions::default(), &base_cache).unwrap();
    active.wait_for_reconciliation().unwrap();
    let before = active.view().unwrap();
    assert_eq!(before.revision(), IndexRevision::INITIAL);
    assert!(before.extended_batches(crate::model::CollectionId(0)).unwrap().is_empty());
    let mut inference = crate::inference::InferenceConfig {
        mode: crate::inference::InferenceMode::AutoIndex,
        ..Default::default()
    };
    inference.policy.min_nonblank_occurrences = 1;
    inference.policy.min_nonblank_records = 1;
    active.start_inference(inference).unwrap();
    active.wait_extended().unwrap();
    let after = active.view().unwrap();
    assert_eq!(after.revision(), before.revision());
    assert!(after.capability_revision() > before.capability_revision());
    assert_eq!(after.repository().options().inference, inference);
    assert!(after.extended_batches(crate::model::CollectionId(0)).unwrap()
        .iter().any(|batch| batch.field_states.contains_key(&vec!["nested".into(), "title".into()])));
    let cache = root.join("inferred.snapshot");
    active.flush_cache(&cache).unwrap();
    let restored = Index::load_cache(&root, definitions, RepositoryOptions {
        inference, ..Default::default()
    }, &cache).unwrap();
    assert!(restored.inference_report("data").unwrap().is_some());
    restored.shutdown().unwrap();
    active.shutdown().unwrap();
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn partial_extended_snapshot_retains_exact_coverage_and_resumes() {
    let root = std::env::temp_dir().join(format!(
        "objekte-partial-extended-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    for name in ["one", "two"] {
        std::fs::create_dir_all(root.join(name)).unwrap();
        std::fs::write(root.join(name).join("a.json"), br#"{"name":"Ada"}"#).unwrap();
    }
    let definitions = ["one", "two"]
        .into_iter()
        .map(|name| {
            let mut definition = CollectionDefinition::folder(name, name, SourceFormat::Json);
            definition
                .schema
                .register_field(crate::reference::FieldDefinition {
                    path: vec!["name".into()],
                    kind: crate::model::LogicalType::String,
                    required: false,
                    nullable: false,
                    indexed: true,
                    multi: false,
                    element_nullable: vec![],
                })
                .unwrap();
            definition
        })
        .collect::<Vec<_>>();
    let options = RepositoryOptions::default();
    let base = Arc::new(
        Repository::build_search_base(
            root.canonicalize().unwrap(),
            definitions.clone(),
            options,
            SourceRegistry::builtin(),
            &|_| {},
            &|| false,
        )
        .unwrap(),
    );
    let published = PublishedIndex::from_repository(
        crate::repository::next_repository_instance(),
        IndexRevision::INITIAL,
        Arc::clone(&base),
    )
    .unwrap();
    let index = Index::from_published(published);
    let versions = index.view().unwrap().0.file_versions.clone();
    let admitted = std::sync::atomic::AtomicUsize::new(0);
    let result = Repository::build_extended_from_base(&base, options, &|_, _| {}, &|| false, &|batch| {
        index
            .publish_extended_batch(&base, &versions, batch)
            .map_err(|error| error.to_string())?;
        admitted.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Err("test barrier after first extended batch".into())
    });
    assert!(result.is_err());
    assert_eq!(admitted.load(std::sync::atomic::Ordering::Relaxed), 1);
    let partial = index.view().unwrap();
    assert_eq!(partial.capability_revision().0, 2);
    assert_eq!(
        partial
            .extended_batches(crate::model::CollectionId(0))
            .unwrap()
            .len(),
        1
    );
    assert!(
        partial
            .extended_batches(crate::model::CollectionId(1))
            .unwrap()
            .is_empty()
    );
    let cache = root.join("partial.snapshot");
    index.flush_cache(&cache).unwrap();
    let (_, saved) = crate::storage::load_lifecycle_snapshot_checked(
        root.canonicalize().unwrap(),
        definitions.clone(),
        options,
        &SourceRegistry::builtin(),
        &cache,
        |_| Ok(()),
    )
    .unwrap();
    assert_eq!(saved.capability_revision, 2);
    assert_eq!(saved.extended_batches[0].len(), 1);
    assert!(saved.extended_batches[1].is_empty());
    let restored = Index::load_cache(&root, definitions, options, &cache).unwrap();
    restored.wait_extended().unwrap();
    restored.wait_for_reconciliation().unwrap();
    assert_eq!(restored.status().unwrap().live_records, 2);
    assert_eq!(restored.view().unwrap().revision(), partial.revision());
    let changed = root.join("one/a.json");
    let handle = restored
        .view()
        .unwrap()
        .file_handle_for_path(&changed)
        .unwrap();
    std::fs::write(&changed, br#"{"name":"Grace"}"#).unwrap();
    restored.update_file(handle, &changed).unwrap();
    let updated = restored.view().unwrap();
    assert!(
        updated
            .extended_batches(crate::model::CollectionId(0))
            .unwrap()
            .is_empty()
    );
    let request = Query {
        collections: vec!["one".into(), "two".into()],
        fts: None,
        filter: Some(crate::query::parse_filter("!(name = \"Ada\")").unwrap()),
        options: QueryOptions {
            sort: vec![],
            limit: 10,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    assert_eq!(updated.search(&request).unwrap().count, Some(1));
    assert_eq!(
        updated.search(&request).unwrap().count,
        updated.search_reference(&request).unwrap().count
    );
    restored.shutdown().unwrap();
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

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
