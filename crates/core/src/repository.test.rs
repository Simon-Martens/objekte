use super::RepositoryOptions;
use super::{CollectionDefinition, Repository};
use crate::inventory::RecordClass;
use crate::query::{Query, QueryOptions};
use crate::source::SourceFormat;
use crate::workload::ReturnMode;

#[test]
fn exported_default_prepared_cache_budget_is_sixteen_mib() {
    let options = RepositoryOptions::default();
    assert_eq!(options.prepared_cache_bytes, 16 * 1024 * 1024);
}

#[test]
fn direct_id_lookup_matches_search_metadata_payload_and_stale_semantics() {
    let root = std::env::temp_dir().join(format!("objekte-id-lookup-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    let path = root.join("data/records.jsonl");
    std::fs::write(&path, b"{\"title\":\"healthy\"}\n{\"title\":\n").unwrap();
    let definition = CollectionDefinition::folder("data", "data", SourceFormat::JsonLines);
    let options = RepositoryOptions {
        snapshot_mode: super::SnapshotMode::IfValid,
        ..Default::default()
    };
    let query = |return_mode| Query {
        collections: vec!["data".into()],
        fts: None,
        filter: None,
        options: QueryOptions {
            sort: vec![],
            limit: 10,
            offset: 0,
            count: true,
            return_mode,
            evidence: false,
        },
    };
    let verify = |repo: &Repository| {
        let locator_hits = repo.search(&query(ReturnMode::Locator)).unwrap().hits;
        assert_eq!(locator_hits.len(), 2);
        assert!(
            repo.get_record("data", "absent", ReturnMode::Locator)
                .unwrap()
                .is_none()
        );
        assert!(
            repo.get_record("unknown", "absent", ReturnMode::Locator)
                .is_err()
        );
        for mode in [ReturnMode::Locator, ReturnMode::Bytes, ReturnMode::Logical] {
            let hits = repo.search(&query(mode.clone())).unwrap().hits;
            for hit in hits {
                assert_eq!(
                    repo.get_record("data", &hit.record_id, mode.clone())
                        .unwrap(),
                    Some(hit)
                );
            }
        }
        let fallback = locator_hits
            .iter()
            .find(|hit| hit.record_class == RecordClass::RawFallback)
            .unwrap();
        let unavailable = repo
            .get_record("data", &fallback.record_id, ReturnMode::Logical)
            .unwrap()
            .unwrap();
        assert!(matches!(
            unavailable.payload,
            Some(super::Payload::LogicalUnavailable { .. })
        ));
    };
    let repo = Repository::open_with_options(&root, vec![definition.clone()], options).unwrap();
    verify(&repo);
    let id = repo.search(&query(ReturnMode::Locator)).unwrap().hits[0]
        .record_id
        .clone();
    let moved = root.join("moved.jsonl");
    std::fs::rename(&path, &moved).unwrap();
    assert!(
        repo.get_record("data", &id, ReturnMode::Locator)
            .unwrap()
            .is_some()
    );
    assert!(repo.get_record("data", &id, ReturnMode::Bytes).is_err());
    std::fs::rename(&moved, &path).unwrap();
    repo.save_default_snapshot_for_test();
    let restored = Repository::open_with_options(&root, vec![definition], options).unwrap();
    assert!(restored.startup().restored);
    verify(&restored);
    std::fs::remove_dir_all(root).unwrap();
}
