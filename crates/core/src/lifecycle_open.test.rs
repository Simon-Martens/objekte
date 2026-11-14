use super::*;
use crate::query::{Query, QueryOptions};
use crate::source::SourceFormat;
use crate::workload::ReturnMode;

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
fn direct_json_build_matches_static_inference_overlap_and_managed_restore() {
    use crate::index_storage::{IndexStorageLocation, IndexLocationResolver};
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
    let policy = IndexLocationResolver {
        location: IndexStorageLocation::CustomDirectory(root.join("index-store")),
    };
    std::fs::create_dir(root.join("index-store")).unwrap();
    let index = Index::open_with_storage(
        &root,
        definitions.clone(),
        options,
        policy.clone(),
        crate::source::SourceRegistry::builtin(),
    )
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
    index.shutdown().unwrap();
    let restored = Index::open_with_storage(
        &root,
        definitions,
        options,
        policy,
        crate::source::SourceRegistry::builtin(),
    )
    .unwrap();
    assert!(restored.restored_from_storage());
    check(&restored);
    restored.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
