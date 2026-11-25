use super::Repository;
use crate::inventory::RecordClass;
use crate::model::{LogicalType, Value};
use crate::query::options::ReturnMode;
use crate::query::{Query, QueryOptions};
use crate::query_execution::streaming::{ExecutionContext, ExecutionLimits};
use crate::query_results::Payload;
use crate::repository::configuration::{CollectionDefinition, RepositoryOptions};
use crate::schema::FieldDefinition;

#[test]
fn direct_id_lookup_matches_search_metadata_payload_and_stale_semantics() {
    let root = std::env::temp_dir().join(format!("objekte-id-lookup-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    let path = root.join("data/records.rows");
    std::fs::write(&path, b"(\"title\"=\"healthy\")\n(\"title\"=\n").unwrap();
    let definition =
        CollectionDefinition::folder("data", "data", crate::source::test_support::line_format());
    let options = RepositoryOptions::default();
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
        let collection = &repo.collections()[0];
        for record in &collection.generation.records {
            let (lookup_id, slot) = collection
                .id_lookup
                .get_key_value(record.id.0.as_ref())
                .unwrap();
            assert_eq!(*slot, record.slot);
            assert!(std::sync::Arc::ptr_eq(&lookup_id.0, &record.id.0));
        }
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
            Some(Payload::LogicalUnavailable { .. })
        ));
    };
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![definition.clone()],
        options,
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    verify(&repo);
    let id = repo.search(&query(ReturnMode::Locator)).unwrap().hits[0]
        .record_id
        .clone();
    let moved = root.join("moved.rows");
    std::fs::rename(&path, &moved).unwrap();
    assert!(
        repo.get_record("data", &id, ReturnMode::Locator)
            .unwrap()
            .is_some()
    );
    assert!(repo.get_record("data", &id, ReturnMode::Bytes).is_err());
    std::fs::rename(&moved, &path).unwrap();
    let restored = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![definition],
        options,
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    verify(&restored);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn exact_count_uses_index_proofs_and_resolves_parameters_and_frozen_time() {
    let root = std::env::temp_dir().join(format!("objekte-exact-count-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (name, fixture) in [("a", r#"("name"="Alice")"#), ("b", r#"("name"="Bob")"#)] {
        std::fs::write(root.join(format!("data/{name}.row")), fixture).unwrap();
    }
    let mut definition = crate::source::test_support::document_folder("data", "data");
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
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![definition],
        RepositoryOptions {
            verified_cache_bytes: 1024 * 1024,
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
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
    let moved = root.join("moved.row");
    std::fs::rename(root.join("data/a.row"), &moved).unwrap();
    assert_eq!(repo.count_exact(&bound("Alice")).unwrap(), 1);
    std::fs::rename(&moved, root.join("data/a.row")).unwrap();

    let macro_query = query("@file.modified >= @todayStart");
    let context = |seconds| {
        ExecutionContext::new(
            crate::query::clock::ClockContext::new(
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
        .push(crate::query::options::SortField::simple("[invalid", false));
    assert!(repo.count_exact(&invalid_sort).is_err());
    std::fs::remove_dir_all(root).unwrap();
}
