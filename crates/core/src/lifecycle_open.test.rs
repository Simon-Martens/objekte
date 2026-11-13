use super::*;
use crate::query::{Query, QueryOptions};
use crate::source::SourceFormat;
use crate::workload::ReturnMode;

#[cfg(feature = "detailed-profiling")]
#[test]
fn incremental_and_restore_profiles_preserve_results_and_report_checked_work() {
    let root = std::env::temp_dir().join(format!(
        "objekte-load-profile-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for number in 0..3 {
        std::fs::write(
            root.join(format!("data/{number}.json")),
            format!(r#"{{"title":"record {number}"}}"#),
        )
        .unwrap();
    }
    let definitions = vec![CollectionDefinition::folder(
        "data",
        "data",
        SourceFormat::Json,
    )];
    let options = RepositoryOptions {
        build_workers: std::num::NonZeroUsize::new(2),
        profile_build: true,
        ..Default::default()
    };
    let task = Index::start_incremental_build(&root, definitions.clone(), options).unwrap();
    let index = task.wait_complete().unwrap();
    let view = index.view().unwrap();
    let build = view.startup_stats().build_profile.as_ref().unwrap();
    assert_eq!(build.work.sampled_files, 1);
    assert_eq!(build.segments.len(), 1);
    assert_eq!(build.segments[0].collection, "data");
    assert_eq!(build.segments[0].files, 3);
    assert_eq!(build.segments[0].records, 3);
    assert!(build.segments[0].seal_ns > 0);
    assert_eq!(build.publication_seal_ns, build.segments[0].seal_ns);
    assert_eq!(build.publication_prepare_ns, build.segments[0].prepare_ns);
    assert_eq!(build.publication_install_ns, build.segments[0].install_ns);
    assert!(build.discovery_catalogue_ns >= build.discovery_enumeration_ns);
    assert_eq!(build.admission.batches, 1);
    assert_eq!(build.admission.files, 3);
    assert_eq!(build.admission.records, 3);
    assert_eq!(
        build.admission.total_ns,
        build.admission.incremental_component_ns() + build.admission.remainder_ns
    );
    assert_eq!(
        build.admission_by_collection["data"].total_ns,
        build.admission.total_ns
    );
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
    let expected = view.search(&query).unwrap();
    let ordinary_task = Index::start_incremental_build(
        &root,
        definitions.clone(),
        RepositoryOptions {
            profile_build: false,
            ..options
        },
    )
    .unwrap();
    let ordinary_build = ordinary_task.wait_complete().unwrap();
    assert!(
        ordinary_build
            .view()
            .unwrap()
            .startup_stats()
            .build_profile
            .is_none()
    );
    assert_eq!(
        ordinary_build.view().unwrap().search(&query).unwrap().hits,
        expected.hits
    );
    let cache = root.join("index.snapshot");
    index.flush_cache(&cache).unwrap();
    drop(view);

    let (profiled, load) =
        Index::load_cache_profiled(&root, definitions.clone(), options, &cache).unwrap();
    assert!(profiled.restored_from_cache());
    assert!(load.total_ns >= load.storage.total_ns);
    assert_eq!(load.storage.sections.len(), definitions.len() + 3);
    assert_eq!(load.segments.len(), 1);
    assert_eq!(load.segments[0].collection, "data");
    assert_eq!(load.segments[0].records, 3);
    assert!(load.storage.sections.iter().all(|section| {
        section.checksum_bytes == section.payload_bytes
            && section.deserialize_bytes == section.payload_bytes
    }));
    assert_eq!(
        profiled.view().unwrap().search(&query).unwrap().hits,
        expected.hits
    );
    let ordinary = Index::load_cache(&root, definitions, options, &cache).unwrap();
    assert_eq!(
        ordinary.view().unwrap().search(&query).unwrap().hits,
        expected.hits
    );
    assert!(
        profiled
            .wait_for_reconciliation()
            .unwrap()
            .unwrap()
            .issues
            .is_empty()
    );
    assert!(
        ordinary
            .wait_for_reconciliation()
            .unwrap()
            .unwrap()
            .issues
            .is_empty()
    );
    ordinary.shutdown().unwrap();
    profiled.shutdown().unwrap();
    ordinary_build.shutdown().unwrap();
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(feature = "detailed-profiling")]
#[test]
fn large_framed_file_admission_has_component_accounting() {
    let root = std::env::temp_dir().join(format!(
        "objekte-large-admission-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let mut source = b"{\"title\":\"".to_vec();
    source.extend(std::iter::repeat_n(b'x', 8 * 1024 * 1024 + 1));
    source.extend_from_slice(b"\"}\n");
    std::fs::write(root.join("data/large.jsonl"), source).unwrap();
    let definitions = vec![CollectionDefinition::folder(
        "data",
        "data",
        SourceFormat::JsonLines,
    )];
    let options = RepositoryOptions {
        build_workers: std::num::NonZeroUsize::new(2),
        profile_build: true,
        ..Default::default()
    };
    let task = Index::start_incremental_build(&root, definitions.clone(), options).unwrap();
    let index = task.wait_complete().unwrap();
    let view = index.view().unwrap();
    let admission = &view
        .startup_stats()
        .build_profile
        .as_ref()
        .unwrap()
        .admission;
    assert!(admission.batches >= 1);
    assert_eq!(admission.files, 1);
    assert_eq!(admission.records, 1);
    assert_eq!(
        admission.total_ns,
        admission.incremental_component_ns() + admission.remainder_ns
    );
    assert_eq!(
        view.startup_stats()
            .build_profile
            .as_ref()
            .unwrap()
            .admission_by_collection["data"]
            .total_ns,
        admission.total_ns
    );
    drop(view);
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(feature = "detailed-profiling")]
#[test]
fn incremental_admission_totals_match_each_collection() {
    let root = std::env::temp_dir().join(format!(
        "objekte-collection-admission-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/one.json"), br#"{"title":"one"}"#).unwrap();
    std::fs::write(root.join("data/two.json"), br#"{"title":"two"}"#).unwrap();
    let definitions = ["first", "second"]
        .into_iter()
        .map(|name| CollectionDefinition::folder(name, "data", SourceFormat::Json))
        .collect();
    let task = Index::start_incremental_build(
        &root,
        definitions,
        RepositoryOptions {
            profile_build: true,
            ..Default::default()
        },
    )
    .unwrap();
    let index = task.wait_complete().unwrap();
    let view = index.view().unwrap();
    let profile = view.startup_stats().build_profile.as_ref().unwrap();
    assert_eq!(profile.admission_by_collection.len(), 2);
    assert_eq!(profile.admission.files, 4);
    assert_eq!(profile.admission.records, 4);
    assert_eq!(
        profile.admission.total_ns,
        profile
            .admission_by_collection
            .values()
            .map(|entry| entry.total_ns)
            .sum::<u64>()
    );
    assert!(profile.admission_by_collection.values().all(|entry| {
        entry.files == 2
            && entry.records == 2
            && entry.total_ns == entry.incremental_component_ns() + entry.remainder_ns
    }));
    drop(view);
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn direct_json_build_matches_static_inference_overlap_and_cache_restore() {
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
    let index =
        Index::build_unsealed_for_legacy_snapshot_tests(&root, definitions.clone(), options)
            .unwrap();
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
        static_repo.build_parses()
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
    let view = index.view().unwrap();
    let segments = view.0.segments.iter().flat_map(|group| group.iter()).collect::<Vec<_>>();
    assert!(!segments.is_empty());
    assert!(segments.iter().all(|segment| {
        segment
            .index
            .raw
            .sealed_reader()
            .is_some_and(|reader| reader.is_constructed())
    }));
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
    let timings = index.timings_ms().unwrap();
    assert!(timings.metadata.is_some());
    assert!(timings.search_available.is_some());
    assert!(timings.full_available.is_some());
    assert_eq!(timings.inference_full_available, None);
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
    let load_timings = restored.timings_ms().unwrap();
    assert!(load_timings.metadata.is_some());
    assert!(load_timings.search_available.is_some());
    assert!(load_timings.full_available.is_some());
    assert_eq!(load_timings.inference_full_available, None);
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
