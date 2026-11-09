use super::{compact_source_parts_with_limits, partition_base_with_limits};
use std::sync::Arc;

use crate::inference::{InferenceConfig, InferenceMode};
use crate::lifecycle::{Index, IndexRevision, PublishedIndex};
use crate::query::{Query, QueryOptions};
use crate::repository::{CollectionDefinition, Repository, RepositoryOptions};
use crate::source::SourceFormat;
use crate::workload::ReturnMode;

#[test]
fn partitions_static_base_without_changing_search_or_file_routing() {
    let root = std::env::temp_dir().join(format!(
        "objekte-partition-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for number in 0..7 {
        std::fs::write(
            root.join(format!("data/{number}.jsonl")),
            format!("{{\"name\":\"item-{number}\"}}\n"),
        )
        .unwrap();
    }
    let repository = Repository::open_with_options(
        &root,
        vec![CollectionDefinition::folder(
            "items",
            "data",
            SourceFormat::JsonLines,
        )],
        RepositoryOptions::default(),
    )
    .unwrap();
    let published = PublishedIndex::from_repository(
        crate::repository::next_repository_instance(),
        IndexRevision::INITIAL,
        Arc::new(repository),
    )
    .unwrap();
    let original = Index::from_published(published.clone());
    let query = Query {
        collections: vec!["items".into()],
        fts: None,
        filter: None,
        options: QueryOptions {
            sort: Vec::new(),
            limit: 100,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    let before = original.view().unwrap().search(&query).unwrap();
    let partitioned = partition_base_with_limits(published, 2, 2).unwrap();
    assert_eq!(
        partitioned.repository.collections()[0]
            .generation
            .records
            .len(),
        0
    );
    assert!(partitioned.segments[0].len() >= 4);
    assert!(
        partitioned.segments[0]
            .iter()
            .all(|part| part.index.records.len() <= 2)
    );
    let converted = Index::from_published(partitioned);
    let after = converted.view().unwrap().search(&query).unwrap();
    assert_eq!(before.count, after.count);
    let paths = |result: crate::repository::SearchResult| {
        result
            .hits
            .into_iter()
            .map(|hit| hit.file_path)
            .collect::<Vec<_>>()
    };
    assert_eq!(paths(before), paths(after));
    for number in 0..7 {
        assert!(
            converted
                .view()
                .unwrap()
                .file_handle_for_path(std::path::Path::new(&format!("data/{number}.jsonl")))
                .is_some()
        );
    }
    converted.shutdown().unwrap();
    original.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn compacts_one_large_file_in_source_free_parts() {
    let root = std::env::temp_dir().join(format!(
        "objekte-partition-large-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let path = root.join("data/many.jsonl");
    let bytes = (0..9)
        .map(|number| format!("{{\"name\":\"item-{number}\"}}\n"))
        .collect::<String>();
    std::fs::write(&path, bytes).unwrap();
    let repository = Repository::open_with_options(
        &root,
        vec![CollectionDefinition::folder(
            "items",
            "data",
            SourceFormat::JsonLines,
        )],
        RepositoryOptions::default(),
    )
    .unwrap();
    let published = PublishedIndex::from_repository(
        crate::repository::next_repository_instance(),
        IndexRevision::INITIAL,
        Arc::new(repository),
    )
    .unwrap();
    let index = Index::from_published(published);
    let view = index.view().unwrap();
    let collection = &view.repository().collections()[0];
    let source =
        crate::lifecycle_compaction::base_as_segment(collection, view.repository().files())
            .unwrap();
    std::fs::remove_file(&path).unwrap();
    let partitioned = partition_base_with_limits((*view.0).clone(), 2, 2).unwrap();
    assert_eq!(partitioned.segments[0].len(), 5);
    assert!(
        partitioned.segments[0]
            .iter()
            .all(|part| part.index.records.len() <= 2)
    );
    let parts =
        compact_source_parts_with_limits(&source, &view.0.active[0], &view.0.catalog, 2, 2, || {
            false
        })
        .unwrap();
    assert_eq!(parts.len(), 5);
    assert!(parts.iter().all(|part| part.records.len() <= 2));
    assert_eq!(
        parts.iter().map(|part| part.records.len()).sum::<usize>(),
        9
    );
    assert!(parts.iter().all(|part| part.files.len() == 1));
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn partition_and_cache_restore_keep_global_inference_diagnostics() {
    let root = std::env::temp_dir().join(format!(
        "objekte-partition-inference-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for number in 0..7 {
        std::fs::write(
            root.join(format!("data/{number}.jsonl")),
            format!("{{\"name\":\"item-{number}\",\"kind\":\"same\"}}\n"),
        )
        .unwrap();
    }
    let definitions = vec![CollectionDefinition::folder(
        "items",
        "data",
        SourceFormat::JsonLines,
    )];
    let options = RepositoryOptions {
        inference: InferenceConfig {
            mode: InferenceMode::AutoIndex,
            ..Default::default()
        },
        ..Default::default()
    };
    let repository = Repository::open_with_options(&root, definitions.clone(), options).unwrap();
    let build_parses = repository.build_parses();
    let build_failed_parses = repository.build_failed_parses();
    let startup = repository.startup().clone();
    assert_eq!(build_parses, 7);
    let report = serde_json::to_value(repository.inference_report("items").unwrap()).unwrap();
    let diagnostics =
        serde_json::to_value(repository.inference_index_diagnostics("items").unwrap()).unwrap();
    assert!(!diagnostics.as_array().unwrap().is_empty());
    let published = PublishedIndex::from_repository(
        crate::repository::next_repository_instance(),
        IndexRevision::INITIAL,
        Arc::new(repository),
    )
    .unwrap();
    let partitioned = partition_base_with_limits(published, 2, 2).unwrap();
    assert!(partitioned.segments[0].len() >= 4);
    let index = Index::from_published(partitioned);
    let check = |index: &Index| {
        let view = index.view().unwrap();
        let repository = view.repository();
        assert_eq!(
            serde_json::to_value(repository.inference_report("items").unwrap()).unwrap(),
            report
        );
        assert_eq!(
            serde_json::to_value(repository.inference_index_diagnostics("items").unwrap()).unwrap(),
            diagnostics
        );
        assert!(
            repository.collections()[0]
                .inference
                .as_ref()
                .unwrap()
                .indexes
                .is_empty()
        );
    };
    check(&index);
    assert_eq!(index.view().unwrap().repository().build_parses(), build_parses);
    assert_eq!(
        index.view().unwrap().repository().build_failed_parses(),
        build_failed_parses
    );
    assert_eq!(
        index.view().unwrap().repository().startup().build_workers,
        startup.build_workers
    );
    let cache = root.join("partition.snapshot");
    index.flush_cache(&cache).unwrap();
    let restored = Index::load_cache(&root, definitions, options, &cache).unwrap();
    check(&restored);
    restored.shutdown().unwrap();
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
