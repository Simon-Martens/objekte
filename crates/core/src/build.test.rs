use super::*;
use crate::model::{FieldPath, LogicalType};
use crate::query::{Query, QueryOptions, parse_filter};
use crate::reference::{CustomBinding, FieldDefinition, Schema, Transform};
use crate::repository::{Repository, RepositoryOptions, SnapshotMode};
use crate::source::{SourceFormat, TextSegment};
use crate::workload::ReturnMode;

#[test]
fn file_batches_bound_count_and_source_bytes_without_losing_empty_files() {
    let mut inventory = FileInventory::new("/synthetic".into());
    let mut ids = Vec::new();
    for index in 0..600 {
        ids.push(
            inventory
                .add(
                    format!("{index:04}.json").into(),
                    SourceRevision {
                        modified_ns: 1,
                        size: 0,
                    },
                    None,
                    CollectionId(0),
                )
                .unwrap(),
        );
    }
    assert_eq!(
        file_batches(&ids, &inventory)
            .iter()
            .map(|batch| batch.len())
            .collect::<Vec<_>>(),
        [512, 88]
    );
    let mut large = FileInventory::new("/synthetic".into());
    let mut ids = Vec::new();
    for (index, size) in [5, 5, 9, 1].into_iter().enumerate() {
        ids.push(
            large
                .add(
                    format!("{index}.json").into(),
                    SourceRevision {
                        modified_ns: 1,
                        size: size * 1024 * 1024,
                    },
                    None,
                    CollectionId(0),
                )
                .unwrap(),
        );
    }
    assert_eq!(
        file_batches(&ids, &large)
            .iter()
            .map(|batch| batch.len())
            .collect::<Vec<_>>(),
        [1, 1, 1, 1]
    );
}

#[test]
fn bounded_batch_admission_keeps_slots_diagnostics_and_complete_gram_maps() {
    let root = std::env::temp_dir().join(format!("objekte-build-batch-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for number in 0..512 {
        std::fs::write(
            root.join(format!("data/{number:04}.jsonl")),
            b"{\"title\":\"x\"}",
        )
        .unwrap();
    }
    std::fs::write(root.join("data/0512.jsonl"), b"{\"title\":").unwrap();
    std::fs::write(root.join("data/0513.jsonl"), b"{\"title\":\"y\"}").unwrap();
    std::fs::write(root.join("data/0514.jsonl"), b"").unwrap();
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["title".into()],
            kind: LogicalType::String,
            required: false,
            nullable: true,
            indexed: true,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    schema
        .register_custom(
            vec!["title".into()],
            CustomBinding {
                name: "folded".into(),
                version: 1,
                transform: Transform::CaseFold,
            },
        )
        .unwrap();
    let definition = CollectionDefinition {
        name: "data".into(),
        source_path: "data".into(),
        format: SourceFormat::JsonLines,
        identity: IdentityPolicy::Filename,
        discovery: Default::default(),
        schema,
        sort_fields: vec![
            FieldPath::parse("title").unwrap(),
            FieldPath::parse("@file.status").unwrap(),
        ],
        sort_locale: "de".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    };
    for alphabet in [BigramAlphabet::Ascii, BigramAlphabet::Unicode] {
        let options = RepositoryOptions {
            bigram_alphabet: alphabet,
            snapshot_mode: SnapshotMode::ReadWrite,
            ..Default::default()
        };
        let built =
            Repository::open_with_options(&root, vec![definition.clone()], options).unwrap();
        let restored =
            Repository::open_with_options(&root, vec![definition.clone()], options).unwrap();
        assert!(restored.startup().restored);
        for repo in [&built, &restored] {
            let collection = &repo.collections()[0];
            assert_eq!(collection.generation.files.len(), 515);
            assert_eq!(collection.generation.records.len(), 514);
            assert_eq!(
                collection.generation.file_records.records_for(FileId(514)),
                Some([].as_slice())
            );
            assert_eq!(repo.files().files[512].status, FileStatus::Broken);
            let malformed = &collection.generation.records[512];
            assert_eq!(malformed.class, RecordClass::RawFallback);
            assert_eq!(
                repo.files()
                    .diagnostic(malformed.file, malformed.diagnostic_ref)
                    .unwrap()
                    .record,
                Some(malformed.slot)
            );
            for (filter, count, first_id) in [
                ("title = 'x'", 512, "0000"),
                ("title = 'y'", 1, "0513"),
                ("@record.class = 1", 1, "~fallback:"),
            ] {
                let query = Query {
                    collections: vec!["data".into()],
                    filter: Some(parse_filter(filter).unwrap()),
                    fts: None,
                    options: QueryOptions {
                        sort: vec![],
                        limit: 3,
                        offset: 0,
                        count: true,
                        return_mode: ReturnMode::Locator,
                        evidence: true,
                    },
                };
                let result = repo.search(&query).unwrap();
                assert_eq!(result.count, Some(count), "{filter}");
                assert!(result.hits[0].record_id.starts_with(first_id), "{filter}");
                assert_eq!(result.counters.files_opened, 0, "{filter}");
                assert_eq!(
                    result.hits,
                    repo.search_reference(&query).unwrap().hits,
                    "{filter}"
                );
            }
            let mut direct = BigramIndex::new(alphabet);
            for slot in 0..514 {
                let text = match slot {
                    0..=511 => Some("x"),
                    513 => Some("y"),
                    _ => None,
                };
                if let Some(text) = text {
                    direct
                        .add_record(
                            RecordSlot(slot),
                            &[TextSegment {
                                path: vec!["title".into()],
                                text: text.into(),
                            }],
                        )
                        .unwrap();
                } else {
                    direct.add_empty(RecordSlot(slot)).unwrap();
                }
            }
            assert_eq!(collection.raw.normal, direct.normal);
            assert_eq!(collection.raw.skip, direct.skip);
            assert_eq!(collection.custom_text.len(), 1);
            let custom = collection.custom_text.values().next().unwrap();
            assert_eq!(custom.normal, direct.normal);
            assert_eq!(custom.skip, direct.skip);
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn shared_file_batches_reconcile_diagnostic_caps_and_healthy_siblings() {
    let root = std::env::temp_dir().join(format!("objekte-build-shared-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let mut contents = "{\"title\":\n".repeat(70);
    contents.push_str("{\"title\":\"ok\"}\n");
    std::fs::write(root.join("data/mixed.jsonl"), contents).unwrap();
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["title".into()],
            kind: LogicalType::String,
            required: false,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    let definitions = ["a", "b"]
        .into_iter()
        .map(|name| CollectionDefinition {
            name: name.into(),
            source_path: "data".into(),
            format: SourceFormat::JsonLines,
            identity: IdentityPolicy::Generated,
            discovery: Default::default(),
            schema: schema.clone(),
            sort_fields: vec![FieldPath::parse("@file.status").unwrap()],
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        })
        .collect::<Vec<_>>();
    let options = RepositoryOptions {
        snapshot_mode: SnapshotMode::ReadWrite,
        ..Default::default()
    };
    let built = Repository::open_with_options(&root, definitions.clone(), options).unwrap();
    let restored = Repository::open_with_options(&root, definitions, options).unwrap();
    assert!(restored.startup().restored);
    for repo in [&built, &restored] {
        let file = &repo.files().files[0];
        assert_eq!(file.status, FileStatus::Broken);
        assert_eq!(file.diagnostics.len(), 64);
        assert_eq!(file.omitted_diagnostics, 76);
        for collection in repo.collections() {
            assert_eq!(collection.generation.records.len(), 71);
            assert_eq!(
                collection
                    .generation
                    .file_records
                    .records_for(FileId(0))
                    .unwrap()
                    .len(),
                71
            );
            assert_eq!(
                collection.generation.records[70].class,
                RecordClass::Logical
            );
        }
        let query = |filter| Query {
            collections: vec![],
            filter: Some(parse_filter(filter).unwrap()),
            fts: None,
            options: QueryOptions {
                sort: vec![],
                limit: 3,
                offset: 0,
                count: true,
                return_mode: ReturnMode::Locator,
                evidence: true,
            },
        };
        for (filter, count) in [
            ("@record.class = 1", 140),
            ("title = 'ok'", 2),
            ("@file.status = 1", 142),
        ] {
            let result = repo.search(&query(filter)).unwrap();
            assert_eq!(result.count, Some(count), "{filter}");
            assert_eq!(result.counters.files_opened, 0, "{filter}");
            assert_eq!(
                result.hits,
                repo.search_reference(&query(filter)).unwrap().hits
            );
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn duplicate_intrinsic_ids_across_batches_fail_before_publication() {
    let root = std::env::temp_dir().join(format!("objekte-build-duplicate-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for number in 0..513 {
        let id = if number == 0 || number == 512 {
            "same".into()
        } else {
            format!("id-{number}")
        };
        std::fs::write(
            root.join(format!("data/{number:04}.jsonl")),
            format!("{{\"id\":\"{id}\"}}\n"),
        )
        .unwrap();
    }
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["id".into()],
            kind: LogicalType::String,
            required: true,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    let error = Repository::open_with_options(
        &root,
        vec![CollectionDefinition {
            name: "data".into(),
            source_path: "data".into(),
            format: SourceFormat::JsonLines,
            identity: IdentityPolicy::Intrinsic(vec!["id".into()]),
            discovery: Default::default(),
            schema,
            sort_fields: vec![],
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        }],
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            ..Default::default()
        },
    )
    .err()
    .unwrap();
    assert!(
        error.contains("duplicate record ID") && error.contains("0512.jsonl"),
        "{error}"
    );
    std::fs::remove_dir_all(root).unwrap();
}
