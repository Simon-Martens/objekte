use super::RepositoryOptions;
use super::{CollectionDefinition, Repository};
use crate::inventory::RecordClass;
use crate::model::{LogicalType, Value};
use crate::query::{Query, QueryOptions};
use crate::reference::FieldDefinition;
use crate::source::SourceFormat;
use crate::streaming::{ExecutionContext, ExecutionLimits};
use crate::workload::ReturnMode;

#[test]
fn fresh_fingerprints_require_every_shared_binding_to_agree() {
    use super::finish_fresh_fingerprints;
    use crate::inventory::{FileInventory, SourceRevision};
    use crate::model::CollectionId;

    let mut inventory = FileInventory::new(std::path::PathBuf::from("/tmp"));
    let revision = SourceRevision {
        modified_ns: 1,
        size: 3,
    };
    let file = inventory
        .add("shared.json".into(), revision, None, CollectionId(0))
        .unwrap();
    assert_eq!(
        inventory
            .add("shared.json".into(), revision, None, CollectionId(1))
            .unwrap(),
        file
    );
    let digest = [7; 32];
    assert_eq!(
        finish_fresh_fingerprints(
            &inventory,
            vec![vec![
                (CollectionId(0), digest),
                (CollectionId(0), digest),
                (CollectionId(1), digest),
            ]],
        )
        .unwrap(),
        [Some(digest)]
    );
    assert_eq!(
        finish_fresh_fingerprints(&inventory, vec![vec![(CollectionId(0), digest)]])
            .unwrap(),
        [None]
    );
    assert!(finish_fresh_fingerprints(
        &inventory,
        vec![vec![(CollectionId(0), digest), (CollectionId(1), [8; 32])]],
    )
    .is_err());
}

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

#[test]
fn exact_count_uses_index_proofs_and_resolves_parameters_and_frozen_time() {
    let root = std::env::temp_dir().join(format!("objekte-exact-count-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (name, json) in [("a", r#"{"name":"Alice"}"#), ("b", r#"{"name":"Bob"}"#)] {
        std::fs::write(root.join(format!("data/{name}.json")), json).unwrap();
    }
    let mut definition = CollectionDefinition::folder("data", "data", SourceFormat::Json);
    definition
        .schema
        .register_field(FieldDefinition {
            path: vec!["name".into()],
            kind: LogicalType::String,
            required: true,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    let repo = Repository::open_with_options(
        &root,
        vec![definition],
        RepositoryOptions {
            verified_cache_bytes: 1024 * 1024,
            ..Default::default()
        },
    )
    .unwrap();
    let query = |filter: &str| Query {
        collections: vec!["data".into()],
        fts: None,
        filter: Some(crate::query::parse_filter(filter).unwrap()),
        options: QueryOptions {
            sort: vec![],
            limit: 1,
            offset: 1,
            count: false,
            return_mode: ReturnMode::Logical,
            evidence: false,
        },
    };
    let template = query("name = $person");
    assert!(repo.count_exact(&template).is_err());
    let bound = |person: &str| {
        template
            .with_parameters(&std::collections::HashMap::from([(
                "person".into(),
                Value::String(person.into()),
            )]))
            .unwrap()
    };
    for (person, expected) in [("Alice", 1), ("nobody", 0), ("Bob", 1), ("Alice", 1)] {
        let request = bound(person);
        assert_eq!(repo.count_exact(&request).unwrap(), expected);
        assert_eq!(
            repo.select_exact("data", &request).unwrap().len(),
            expected as u64
        );
        let mut direct = request.clone();
        direct.options.count = true;
        direct.options.limit = 0;
        let result = repo.search(&direct).unwrap();
        assert_eq!(result.count, Some(expected));
        assert!(result.hits.is_empty());
        assert_eq!(result.counters.files_opened, 0);
        assert_eq!(result.counters.scanned_records, 0);
        assert_eq!(result.counters.sort_key_evaluations, 0);
    }
    let moved = root.join("moved.json");
    std::fs::rename(root.join("data/a.json"), &moved).unwrap();
    assert_eq!(repo.count_exact(&bound("Alice")).unwrap(), 1);
    std::fs::rename(&moved, root.join("data/a.json")).unwrap();

    let macro_query = query("@file.modified >= @todayStart");
    let context = |seconds| {
        ExecutionContext::new(
            crate::clock_context::ClockContext::new(
                time::OffsetDateTime::from_unix_timestamp(seconds).unwrap(),
            ),
            ExecutionLimits::default(),
            None,
        )
    };
    assert_eq!(
        repo.count_exact_with_context(&macro_query, &context(946684800))
            .unwrap(),
        2
    );
    assert_eq!(
        repo.count_exact_with_context(&macro_query, &context(2524608000))
            .unwrap(),
        0
    );
    assert_eq!(
        repo.count_exact_with_context(&macro_query, &context(946684800))
            .unwrap(),
        2
    );
    let mut invalid_sort = bound("Alice");
    invalid_sort
        .options
        .sort
        .push(crate::workload::SortField::simple("[invalid", false));
    assert!(repo.count_exact(&invalid_sort).is_err());
    std::fs::remove_dir_all(root).unwrap();
}
