use objekte_search_core::lifecycle::Index;
use objekte_search_core::lifecycle::{CompactionMode, CompactionOptions};
use objekte_search_core::model::LogicalType;
use objekte_search_core::query::{
    CaseSensitivity, FtsMode, FtsQuery, Query, QueryOptions, parse_filter,
};
use objekte_search_core::reference::FieldDefinition;
use objekte_search_core::relation::{RelationCardinality, RelationDefinition};
use objekte_search_core::repository::{
    CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions,
};
use objekte_search_core::source::SourceFormat;
use objekte_search_core::storage::{
    AuditOptions, CachePolicy, StorageOpenOptions, StorageReadLimits, StorageWriteOptions, audit,
    prune,
};
use objekte_search_core::workload::{ReturnMode, SortField};
use sha2::{Digest, Sha256};

fn fixture_root(label: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!(
        "objekte-storage-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::create_dir_all(root.join("storage")).unwrap();
    std::fs::write(
        root.join("data/a.json"),
        br#"{"id":"a","title":"Alpha token"}"#,
    )
    .unwrap();
    std::fs::write(
        root.join("data/b.json"),
        br#"{"id":"b","title":"Beta token"}"#,
    )
    .unwrap();
    root
}

fn copy_tree(source: &std::path::Path, target: &std::path::Path) {
    std::fs::create_dir_all(target).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let destination = target.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &destination);
        } else {
            std::fs::copy(entry.path(), destination).unwrap();
        }
    }
}

fn definitions() -> Vec<CollectionDefinition> {
    let mut definition = CollectionDefinition::folder("items", "data", SourceFormat::Json);
    for (name, kind) in [("id", LogicalType::Id), ("title", LogicalType::String)] {
        definition
            .schema
            .register_field(FieldDefinition {
                path: vec![name.into()],
                kind,
                required: false,
                nullable: false,
                indexed: true,
                multi: false,
                element_nullable: Vec::new(),
            })
            .unwrap();
    }
    definition
        .sort_fields
        .push(objekte_search_core::model::FieldPath::parse("title").unwrap());
    vec![definition]
}

fn all_records() -> Query {
    Query {
        collections: vec!["items".into()],
        fts: None,
        filter: None,
        options: QueryOptions {
            sort: Vec::new(),
            limit: 20,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    }
}

fn relation_definitions() -> Vec<CollectionDefinition> {
    fn add_field(definition: &mut CollectionDefinition, name: &str, kind: LogicalType) {
        definition
            .schema
            .register_field(FieldDefinition {
                path: vec![name.into()],
                kind,
                required: name == "id",
                nullable: name != "id",
                indexed: true,
                multi: false,
                element_nullable: Vec::new(),
            })
            .unwrap();
    }

    let mut entries = CollectionDefinition::folder("entries", "entries", SourceFormat::Json);
    entries.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
    add_field(&mut entries, "id", LogicalType::Id);
    add_field(&mut entries, "title", LogicalType::String);

    let mut mentions = CollectionDefinition::folder("mentions", "mentions", SourceFormat::Json);
    mentions.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
    add_field(&mut mentions, "id", LogicalType::Id);
    add_field(&mut mentions, "title", LogicalType::String);
    add_field(&mut mentions, "entry", LogicalType::Id);
    mentions
        .schema
        .register_relation(RelationDefinition {
            path: vec!["entry".into()],
            target_collection: "entries".into(),
            cardinality: RelationCardinality::One,
            reverse_alias: Some("mentions".into()),
        })
        .unwrap();
    vec![entries, mentions]
}

fn semantic_definitions() -> Vec<CollectionDefinition> {
    let mut definition = CollectionDefinition::folder("items", "data", SourceFormat::Json);
    definition.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
    for (path, kind, multi) in [
        ("id", LogicalType::Id, false),
        ("title", LogicalType::String, false),
        ("amount", LogicalType::Number, false),
        (
            "tags",
            LogicalType::Array(Box::new(LogicalType::String)),
            true,
        ),
    ] {
        definition
            .schema
            .register_field(FieldDefinition {
                path: vec![path.into()],
                kind,
                required: path == "id",
                nullable: path == "amount",
                indexed: true,
                multi,
                element_nullable: Vec::new(),
            })
            .unwrap();
    }
    for field in ["title", "amount"] {
        definition
            .sort_fields
            .push(objekte_search_core::model::FieldPath::parse(field).unwrap());
    }
    definition.schema.composite_sorts.push(vec![
        SortField::simple("title", false),
        SortField::simple("amount", true),
    ]);
    vec![definition]
}

fn sequence_definitions() -> Vec<CollectionDefinition> {
    let mut definition = CollectionDefinition::folder("items", "data", SourceFormat::JsonLines);
    definition.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
    for (path, kind) in [("id", LogicalType::Id), ("title", LogicalType::String)] {
        definition
            .schema
            .register_field(FieldDefinition {
                path: vec![path.into()],
                kind,
                required: path == "id",
                nullable: false,
                indexed: true,
                multi: false,
                element_nullable: Vec::new(),
            })
            .unwrap();
    }
    definition
        .sort_fields
        .push(objekte_search_core::model::FieldPath::parse("title").unwrap());
    vec![definition]
}

#[test]
fn reopened_semantic_query_matrix_matches_native_storage_state() {
    let root = fixture_root("semantic-matrix");
    let documents = [
        (
            "a.json",
            r#"{"id":"a","title":"Straße","amount":9007199254740993.1,"tags":["α","β"]}"#,
        ),
        (
            "b.json",
            r#"{"id":"b","title":"STRASSE","amount":null,"tags":[]}"#,
        ),
        ("c.json", r#"{"id":"c","title":"Tie","amount":2}"#),
        (
            "d.json",
            r#"{"id":"d","title":"Tie","amount":1.00000000000000000001,"tags":["β"]}"#,
        ),
    ];
    for (name, document) in documents {
        std::fs::write(root.join("data").join(name), document).unwrap();
    }
    let definitions = semantic_definitions();
    let index = Index::build(&root, definitions.clone(), RepositoryOptions::default()).unwrap();
    let mut queries = Vec::new();
    for filter in [
        "title >= 'Tie'",
        "amount = 9007199254740993.1",
        "amount > 1",
        "amount:null = true",
        "amount:exists = false",
        "tags = 'β'",
    ] {
        let mut request = all_records();
        request.filter = Some(parse_filter(filter).unwrap());
        queries.push(request);
    }
    let mut fts = all_records();
    fts.fts = Some(FtsQuery {
        case_sensitivity: CaseSensitivity::Insensitive,
        mode: FtsMode::Literal,
        literal: "strasse".into(),
        pattern_parameter: None,
        fields: Vec::new(),
    });
    queries.push(fts);
    for (field, descending) in [("title", false), ("amount", true)] {
        let mut request = all_records();
        request.options.sort = vec![SortField::simple(field, descending)];
        queries.push(request);
    }
    let mut composite = all_records();
    composite.options.sort = vec![
        SortField::simple("title", false),
        SortField::simple("amount", true),
    ];
    queries.push(composite);
    let native = queries
        .iter()
        .map(|request| index.view().unwrap().search(request).unwrap())
        .collect::<Vec<_>>();
    index
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    drop(index);
    let reopened = Index::open_storage(
        &root,
        definitions,
        RepositoryOptions::default(),
        root.join("storage"),
        StorageOpenOptions::default(),
    )
    .unwrap();
    for (request, expected) in queries.iter().zip(native) {
        let actual = reopened.view().unwrap().search(request).unwrap();
        assert_eq!(actual.count, expected.count, "{request:?}");
        assert_eq!(actual.hits, expected.hits, "{request:?}");
    }
    reopened.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn native_build_and_memory_archive_do_not_touch_storage_files() {
    let root = fixture_root("memory-only");
    std::fs::remove_dir_all(root.join("storage")).unwrap();
    let index = Index::build(&root, definitions(), RepositoryOptions::default()).unwrap();
    assert!(!root.join("storage").exists());
    assert_eq!(index.storage_usage().unwrap().archive.serialized_bytes, 0);
    let before = index.view().unwrap().search(&all_records()).unwrap();
    index.archive().unwrap();
    assert!(!root.join("storage").exists());
    let after = index.view().unwrap().search(&all_records()).unwrap();
    assert_eq!(after.count, before.count);
    assert_eq!(
        after
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        before
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>()
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn explicit_save_reopens_a_complete_searchable_snapshot() {
    let root = fixture_root("roundtrip");
    let index = Index::build(&root, definitions(), RepositoryOptions::default()).unwrap();
    let before = index.view().unwrap().search(&all_records()).unwrap();
    let report = index
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    assert!(!report.unchanged);
    let repeated = index
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    assert!(repeated.unchanged);
    assert_eq!(repeated.objects_new, 0);
    assert_eq!(repeated.bytes_serialized, 0);
    assert_eq!(repeated.bytes_written, 0);
    assert_eq!(repeated.sync_calls, 0);
    drop(index);

    let reopened = Index::open_storage(
        &root,
        definitions(),
        RepositoryOptions::default(),
        root.join("storage"),
        StorageOpenOptions::default(),
    )
    .unwrap();
    let after = reopened.view().unwrap().search(&all_records()).unwrap();
    assert_eq!(after.count, before.count);
    assert_eq!(
        after
            .hits
            .into_iter()
            .map(|hit| hit.record_id)
            .collect::<Vec<_>>(),
        before
            .hits
            .into_iter()
            .map(|hit| hit.record_id)
            .collect::<Vec<_>>()
    );
    reopened.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn open_stays_payload_lazy_until_an_indexed_filter_selects_a_posting() {
    let root = fixture_root("lazy");
    let index = Index::build(&root, definitions(), RepositoryOptions::default()).unwrap();
    index
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    let mut native_sorted = all_records();
    native_sorted.options.sort = vec![SortField::simple("title", false)];
    assert_eq!(
        index.view().unwrap().search(&native_sorted).unwrap().count,
        Some(2)
    );

    drop(index);

    let reopened = Index::open_storage(
        &root,
        definitions(),
        RepositoryOptions::default(),
        root.join("storage"),
        StorageOpenOptions::default(),
    )
    .unwrap();
    let cold = reopened.storage_status().unwrap();
    assert_eq!(cold.index_reads, 2);

    let mut request = all_records();
    request.filter = Some(parse_filter("title = \"Alpha token\"").unwrap());
    let result = reopened.view().unwrap().search(&request).unwrap();
    assert_eq!(result.count, Some(1));
    let warm = reopened.storage_status().unwrap();
    assert!(warm.index_reads > cold.index_reads);
    assert_eq!(
        warm.cache.loaded_families, 1,
        "an equality filter must retain only its selected posting leaf"
    );

    reopened.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn presence_and_null_queries_do_not_open_value_or_text_payloads() {
    let root = fixture_root("presence-null-state");
    let index = Index::build(&root, definitions(), RepositoryOptions::default()).unwrap();
    index
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    drop(index);
    let reopened = Index::open_storage(
        &root,
        definitions(),
        RepositoryOptions::default(),
        root.join("storage"),
        StorageOpenOptions::default(),
    )
    .unwrap();
    let cold = reopened.storage_status().unwrap();
    let mut exists = all_records();
    exists.filter = Some(parse_filter("title:exists = true").unwrap());
    assert_eq!(
        reopened.view().unwrap().search(&exists).unwrap().count,
        Some(2)
    );
    assert_eq!(
        reopened.storage_status().unwrap().index_reads,
        cold.index_reads,
        "presence must use the eager state masks"
    );
    let mut null = all_records();
    null.filter = Some(parse_filter("title:null = true").unwrap());
    assert_eq!(
        reopened.view().unwrap().search(&null).unwrap().count,
        Some(0)
    );
    assert_eq!(
        reopened.storage_status().unwrap().index_reads,
        cold.index_reads,
        "null must use the eager state masks"
    );
    reopened.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn configured_sort_frames_load_only_when_sort_execution_needs_them() {
    let root = fixture_root("sort-lazy");
    let index = Index::build(&root, definitions(), RepositoryOptions::default()).unwrap();
    index
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    drop(index);

    let reopened = Index::open_storage(
        &root,
        definitions(),
        RepositoryOptions::default(),
        root.join("storage"),
        StorageOpenOptions::default(),
    )
    .unwrap();
    let cold = reopened.storage_status().unwrap();
    let mut request = all_records();
    request.options.sort = vec![SortField::simple("title", false)];
    request.options.count = false;
    let result = reopened.view().unwrap().search(&request).unwrap();
    assert_eq!(
        result
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["path:a.json", "path:b.json"]
    );
    let warm = reopened.storage_status().unwrap();
    assert!(warm.index_reads > cold.index_reads);
    reopened.view().unwrap().search(&request).unwrap();
    assert_eq!(
        reopened.storage_status().unwrap().index_reads,
        warm.index_reads
    );
    reopened.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn explain_does_not_read_selected_lazy_payloads() {
    let root = fixture_root("explain-lazy");
    let index = Index::build(&root, definitions(), RepositoryOptions::default()).unwrap();
    index
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    drop(index);
    let reopened = Index::open_storage(
        &root,
        definitions(),
        RepositoryOptions::default(),
        root.join("storage"),
        StorageOpenOptions::default(),
    )
    .unwrap();
    let cold = reopened.storage_status().unwrap();
    for entry in std::fs::read_dir(root.join("storage/objects")).unwrap() {
        let path = entry.unwrap().path();
        if !path
            .extension()
            .is_some_and(|extension| extension == "pack")
        {
            continue;
        }
        let mut bytes = std::fs::read(&path).unwrap();
        let kind = u16::from_le_bytes(bytes[10..12].try_into().unwrap());
        if matches!(kind, 3..=5 | 8 | 10 | 11) {
            for byte in &mut bytes[64..] {
                *byte ^= 0xff;
            }
            std::fs::write(path, bytes).unwrap();
        }
    }
    let mut request = all_records();
    request.filter = Some(parse_filter("title = \"Alpha token\"").unwrap());
    request.options.sort = vec![SortField::simple("title", false)];
    request.fts = Some(FtsQuery {
        case_sensitivity: CaseSensitivity::Sensitive,
        mode: FtsMode::Literal,
        literal: "token".into(),
        pattern_parameter: None,
        fields: Vec::new(),
    });
    let traces = reopened.view().unwrap().explain(&request).unwrap();
    assert_eq!(traces.len(), 1);
    assert_eq!(
        reopened.storage_status().unwrap().index_reads,
        cold.index_reads
    );
    reopened.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn selected_sort_loads_only_its_configured_column_frame() {
    let root = fixture_root("sort-column-lazy");
    let mut definitions = definitions();
    definitions[0]
        .sort_fields
        .push(objekte_search_core::model::FieldPath::parse("id").unwrap());
    let index = Index::build(&root, definitions.clone(), RepositoryOptions::default()).unwrap();
    index
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    drop(index);
    let reopened = Index::open_storage(
        &root,
        definitions,
        RepositoryOptions::default(),
        root.join("storage"),
        StorageOpenOptions::default(),
    )
    .unwrap();
    let cold = reopened.storage_status().unwrap();
    let mut by_title = all_records();
    by_title.options.sort = vec![SortField::simple("title", false)];
    by_title.options.count = false;
    reopened.view().unwrap().search(&by_title).unwrap();
    let first = reopened.storage_status().unwrap();
    assert!(first.index_reads > cold.index_reads);
    assert!(
        first.cache.loaded_families <= 2,
        "title must not load the configured id column: {:?}",
        first.cache
    );

    let mut by_id = all_records();
    by_id.options.sort = vec![SortField::simple("id", false)];
    by_id.options.count = false;
    reopened.view().unwrap().search(&by_id).unwrap();
    let second = reopened.storage_status().unwrap();
    assert!(second.index_reads > first.index_reads);
    assert!(second.cache.loaded_families > first.cache.loaded_families);
    reopened.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn corrupt_selected_sort_errors_instead_of_falling_back_to_source() {
    let root = fixture_root("sort-corrupt");
    let index = Index::build(&root, definitions(), RepositoryOptions::default()).unwrap();
    index
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    drop(index);
    let sort_object = object_with_kind(&root.join("storage"), 10);
    let mut bytes = std::fs::read(&sort_object).unwrap();
    bytes.truncate(bytes.len() - 1);
    std::fs::write(sort_object, bytes).unwrap();

    let reopened = Index::open_storage(
        &root,
        definitions(),
        RepositoryOptions::default(),
        root.join("storage"),
        StorageOpenOptions::default(),
    )
    .unwrap();
    assert_eq!(
        reopened
            .view()
            .unwrap()
            .search(&all_records())
            .unwrap()
            .count,
        Some(2)
    );
    let mut sorted = all_records();
    sorted.options.sort = vec![SortField::simple("title", false)];
    assert!(reopened.view().unwrap().search(&sorted).is_err());
    reopened.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn add_and_delete_after_reopen_publish_a_new_storage_generation() {
    let root = fixture_root("mutation");
    let index = Index::build(&root, definitions(), RepositoryOptions::default()).unwrap();
    let first = index
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    drop(index);

    let reopened = Index::open_storage(
        &root,
        definitions(),
        RepositoryOptions::default(),
        root.join("storage"),
        StorageOpenOptions::default(),
    )
    .unwrap();
    let added_path = root.join("data/c.json");
    std::fs::write(&added_path, br#"{"id":"c","title":"Gamma token"}"#).unwrap();
    reopened.add_file(&added_path).unwrap();
    assert_eq!(
        reopened
            .view()
            .unwrap()
            .search(&all_records())
            .unwrap()
            .count,
        Some(3)
    );
    let second = reopened
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    assert!(!second.unchanged);
    assert_eq!(second.generation, first.generation + 1);

    let handle = reopened
        .view()
        .unwrap()
        .file_handle_for_path(&added_path)
        .unwrap();
    reopened.delete_file(handle).unwrap();
    assert_eq!(
        reopened
            .view()
            .unwrap()
            .search(&all_records())
            .unwrap()
            .count,
        Some(2)
    );
    let third = reopened
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    assert_eq!(third.generation, second.generation + 1);
    reopened.shutdown().unwrap();

    let reopened_again = Index::open_storage(
        &root,
        definitions(),
        RepositoryOptions::default(),
        root.join("storage"),
        StorageOpenOptions::default(),
    )
    .unwrap();
    assert_eq!(
        reopened_again
            .view()
            .unwrap()
            .search(&all_records())
            .unwrap()
            .count,
        Some(2)
    );
    reopened_again.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn deletion_only_save_does_not_read_or_rewrite_unchanged_payload_packs() {
    let root = fixture_root("deletion-no-pack-io");
    let index = Index::build(&root, definitions(), RepositoryOptions::default()).unwrap();
    let first = index
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    drop(index);

    let reopened = Index::open_storage(
        &root,
        definitions(),
        RepositoryOptions::default(),
        root.join("storage"),
        StorageOpenOptions::default(),
    )
    .unwrap();
    let pack = std::fs::read_dir(root.join("storage/objects"))
        .unwrap()
        .map(Result::unwrap)
        .find(|entry| {
            let path = entry.path();
            if !path
                .extension()
                .is_some_and(|extension| extension == "pack")
            {
                return false;
            }
            let bytes = std::fs::read(path).unwrap();
            matches!(
                u16::from_le_bytes(bytes[10..12].try_into().unwrap()),
                3 | 4 | 5
            )
        })
        .map(|entry| entry.path())
        .expect("fixture contains a payload pack");
    let mut corrupted = std::fs::read(&pack).unwrap();
    for byte in &mut corrupted[64..] {
        *byte ^= 0xff;
    }
    std::fs::write(&pack, &corrupted).unwrap();

    let handle = reopened
        .view()
        .unwrap()
        .file_handle_for_path(&root.join("data/a.json"))
        .unwrap();
    reopened.delete_file(handle).unwrap();
    let report = reopened
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();

    assert_eq!(report.generation, first.generation + 1);
    assert_eq!(std::fs::read(&pack).unwrap(), corrupted);
    reopened.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

fn object_with_kind(directory: &std::path::Path, kind: u16) -> std::path::PathBuf {
    std::fs::read_dir(directory.join("objects"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            let bytes = std::fs::read(path).unwrap();
            bytes.len() >= 12 && u16::from_le_bytes(bytes[10..12].try_into().unwrap()) == kind
        })
        .unwrap()
}

#[test]
fn audit_detects_corrupt_lazy_payload_and_prune_respects_reader_leases() {
    let root = fixture_root("audit");
    let index = Index::build(&root, definitions(), RepositoryOptions::default()).unwrap();
    index
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    drop(index);

    let posting_object = object_with_kind(&root.join("storage"), 3);
    let mut bytes = std::fs::read(&posting_object).unwrap();
    bytes.truncate(bytes.len() - 1);
    std::fs::write(&posting_object, bytes).unwrap();
    let orphan = root.join(format!("storage/objects/{}.pack", "a".repeat(64)));
    std::fs::write(&orphan, b"unreferenced").unwrap();
    let unrelated = root.join("storage/objects/README");
    std::fs::write(&unrelated, b"not owned by the backend").unwrap();
    let staging = root.join("storage/staging");
    let stale_staging = staging.join(format!("object-{}-1-1.tmp", "b".repeat(64)));
    let unrelated_staging = staging.join("operator-notes.tmp");
    std::fs::write(&stale_staging, b"interrupted save").unwrap();
    std::fs::write(&unrelated_staging, b"not backend-owned").unwrap();

    let reopened = Index::open_storage(
        &root,
        definitions(),
        RepositoryOptions::default(),
        root.join("storage"),
        StorageOpenOptions::default(),
    )
    .unwrap();
    let mut request = all_records();
    request.filter = Some(parse_filter("title = \"Alpha token\"").unwrap());
    assert!(reopened.view().unwrap().search(&request).is_err());

    let audit_report = audit(&root.join("storage"), AuditOptions::default()).unwrap();
    assert!(!audit_report.errors.is_empty());
    assert!(prune(&root.join("storage")).is_err());
    reopened.shutdown().unwrap();
    drop(reopened);

    let removed = prune(&root.join("storage")).unwrap();
    assert!(removed.removed_objects >= 1);
    assert!(!orphan.exists());
    assert!(unrelated.exists());
    assert!(!stale_staging.exists());
    assert!(unrelated_staging.exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn prune_deletes_nothing_when_current_manifest_is_corrupt() {
    let root = fixture_root("prune-corrupt-manifest");
    let index = Index::build(&root, definitions(), RepositoryOptions::default()).unwrap();
    index
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    drop(index);
    let manifest = object_with_kind(&root.join("storage"), 1);
    let mut bytes = std::fs::read(&manifest).unwrap();
    bytes[65] ^= 0xff;
    std::fs::write(&manifest, bytes).unwrap();
    let orphan = root.join(format!("storage/objects/{}.pack", "c".repeat(64)));
    std::fs::write(&orphan, b"unreferenced").unwrap();

    assert!(prune(&root.join("storage")).is_err());
    assert!(orphan.exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn full_compaction_after_reopen_does_not_parse_source_files() {
    let root = fixture_root("source-free-compaction");
    let index = Index::build(&root, definitions(), RepositoryOptions::default()).unwrap();
    index
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    drop(index);

    let live_data = root.join("data");
    let offline_data = root.join("data.offline");
    std::fs::rename(&live_data, &offline_data).unwrap();
    let reopened = Index::open_storage(
        &root,
        definitions(),
        RepositoryOptions::default(),
        root.join("storage"),
        StorageOpenOptions::default(),
    )
    .unwrap();
    let cold = reopened.storage_status().unwrap();
    let report = reopened
        .compact_with_options(CompactionOptions {
            mode: CompactionMode::Full,
            merge_small_segments: true,
        })
        .unwrap();
    assert!(report.segments_after <= report.segments_before);
    assert_eq!(
        reopened
            .view()
            .unwrap()
            .search(&all_records())
            .unwrap()
            .count,
        Some(2)
    );
    let after = reopened.storage_status().unwrap();
    assert_eq!(after.source_reads, cold.source_reads);
    assert_eq!(after.source_parses, cold.source_parses);
    reopened.shutdown().unwrap();
    std::fs::rename(&offline_data, &live_data).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn replace_and_move_after_reopen_persist_correct_identity_and_content() {
    let root = fixture_root("replace-move");
    let index = Index::build(&root, definitions(), RepositoryOptions::default()).unwrap();
    index
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    drop(index);

    let reopened = Index::open_storage(
        &root,
        definitions(),
        RepositoryOptions::default(),
        root.join("storage"),
        StorageOpenOptions::default(),
    )
    .unwrap();
    let original = root.join("data/a.json");
    let handle = reopened
        .view()
        .unwrap()
        .file_handle_for_path(&original)
        .unwrap();
    std::fs::write(&original, br#"{"id":"a","title":"Omega replacement"}"#).unwrap();
    reopened.update_file(handle, &original).unwrap();
    let mut replacement_query = all_records();
    replacement_query.filter = Some(parse_filter("title = \"Omega replacement\"").unwrap());
    assert_eq!(
        reopened
            .view()
            .unwrap()
            .search(&replacement_query)
            .unwrap()
            .count,
        Some(1)
    );

    let destination = root.join("data/renamed.json");
    std::fs::rename(&original, &destination).unwrap();
    let moved_handle = reopened
        .view()
        .unwrap()
        .file_handle_for_path(&original)
        .unwrap();
    reopened.move_file(moved_handle, &destination).unwrap();
    reopened
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    reopened.shutdown().unwrap();
    drop(reopened);

    let reopened_again = Index::open_storage(
        &root,
        definitions(),
        RepositoryOptions::default(),
        root.join("storage"),
        StorageOpenOptions::default(),
    )
    .unwrap();
    assert_eq!(
        reopened_again
            .view()
            .unwrap()
            .search(&replacement_query)
            .unwrap()
            .count,
        Some(1)
    );
    assert!(
        reopened_again
            .view()
            .unwrap()
            .file_handle_for_path(&destination)
            .is_some()
    );
    reopened_again.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn independent_writers_cannot_overwrite_a_nonempty_store() {
    let root = fixture_root("writer-conflict");
    let first = Index::build(&root, definitions(), RepositoryOptions::default()).unwrap();
    first
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    drop(first);

    let second = Index::build(&root, definitions(), RepositoryOptions::default()).unwrap();
    let error = second
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap_err();
    assert!(
        error.to_string().contains("storage conflict"),
        "unexpected conflict error: {error}"
    );
    let reopened = Index::open_storage(
        &root,
        definitions(),
        RepositoryOptions::default(),
        root.join("storage"),
        StorageOpenOptions::default(),
    )
    .unwrap();
    let no_change = reopened
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    assert!(no_change.unchanged);
    reopened.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn corrupt_current_and_missing_lazy_objects_have_distinct_outcomes() {
    let root = fixture_root("corruption");
    let index = Index::build(&root, definitions(), RepositoryOptions::default()).unwrap();
    index
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    drop(index);

    let current_path = root.join("storage/CURRENT");
    let current = std::fs::read(&current_path).unwrap();
    std::fs::write(&current_path, [b'X'; 128]).unwrap();
    let error = Index::open_storage(
        &root,
        definitions(),
        RepositoryOptions::default(),
        root.join("storage"),
        StorageOpenOptions::default(),
    )
    .err()
    .expect("bad CURRENT must fail");
    assert!(error.to_string().contains("CURRENT"));
    std::fs::write(&current_path, current).unwrap();

    let posting = object_with_kind(&root.join("storage"), 3);
    std::fs::remove_file(&posting).unwrap();
    let reopened = Index::open_storage(
        &root,
        definitions(),
        RepositoryOptions::default(),
        root.join("storage"),
        StorageOpenOptions::default(),
    )
    .unwrap();
    let mut request = all_records();
    request.filter = Some(parse_filter("title = \"Alpha token\"").unwrap());
    let error = reopened
        .view()
        .unwrap()
        .search(&request)
        .expect_err("a missing selected payload must fail the query");
    assert!(error.contains("No such file") || error.contains("cannot find"));
    reopened.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn lazy_payload_read_limits_fail_at_selection_not_open() {
    let root = fixture_root("limits");
    let index = Index::build(&root, definitions(), RepositoryOptions::default()).unwrap();
    index
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    drop(index);

    let reopened = Index::open_storage(
        &root,
        definitions(),
        RepositoryOptions::default(),
        root.join("storage"),
        StorageOpenOptions {
            limits: StorageReadLimits {
                lazy_dictionary_body_bytes: 1,
                ..StorageReadLimits::default()
            },
            ..StorageOpenOptions::default()
        },
    )
    .unwrap();
    let mut request = all_records();
    request.filter = Some(parse_filter("title = \"Alpha token\"").unwrap());
    let error = reopened
        .view()
        .unwrap()
        .search(&request)
        .expect_err("an oversized selected frame must fail");
    assert!(error.contains("limit is 1"), "{error}");
    reopened.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn current_header_rejects_magic_version_flags_length_and_checksum_errors() {
    let root = fixture_root("current-matrix");
    let index = Index::build(&root, definitions(), RepositoryOptions::default()).unwrap();
    index
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    drop(index);
    let original = std::fs::read(root.join("storage/CURRENT")).unwrap();
    for case in 0..5 {
        let clone = root.join(format!("clone-{case}"));
        copy_tree(&root.join("storage"), &clone);
        let mut current = original.clone();
        match case {
            0 => current[..8].copy_from_slice(b"BADMAGIC"),
            1 => current[8..12].copy_from_slice(&2u32.to_le_bytes()),
            2 => current[12..16].copy_from_slice(&1u32.to_le_bytes()),
            3 => current[56..64].copy_from_slice(&1u64.to_le_bytes()),
            4 => current[32] ^= 0xff,
            _ => unreachable!(),
        }
        if case != 4 {
            let checksum = Sha256::digest(&current[..96]);
            current[96..].copy_from_slice(&checksum);
        }
        std::fs::write(clone.join("CURRENT"), current).unwrap();
        assert!(
            Index::open_storage(
                &root,
                definitions(),
                RepositoryOptions::default(),
                &clone,
                StorageOpenOptions::default(),
            )
            .is_err(),
            "CURRENT corruption case {case} was accepted"
        );
        std::fs::remove_dir_all(clone).unwrap();
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn corrupt_eager_catalog_chunk_fails_open_before_publication() {
    let root = fixture_root("catalog-corrupt");
    let index = Index::build(&root, definitions(), RepositoryOptions::default()).unwrap();
    index
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    drop(index);
    let chunk = object_with_kind(&root.join("storage"), 6);
    let mut bytes = std::fs::read(&chunk).unwrap();
    bytes[64] ^= 0xff;
    std::fs::write(chunk, bytes).unwrap();
    assert!(
        Index::open_storage(
            &root,
            definitions(),
            RepositoryOptions::default(),
            root.join("storage"),
            StorageOpenOptions::default(),
        )
        .is_err()
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn explicit_cache_trim_releases_serialized_staging_entries() {
    let root = fixture_root("trim");
    let index = Index::build(&root, definitions(), RepositoryOptions::default()).unwrap();
    index
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    drop(index);
    let reopened = Index::open_storage(
        &root,
        definitions(),
        RepositoryOptions::default(),
        root.join("storage"),
        StorageOpenOptions::default(),
    )
    .unwrap();
    let mut request = all_records();
    request.filter = Some(parse_filter("title = \"Alpha token\"").unwrap());
    reopened.view().unwrap().search(&request).unwrap();
    assert!(reopened.storage_status().unwrap().cache.resident_bytes > 0);
    let trimmed = reopened.trim_storage_cache().unwrap();
    assert!(trimmed.evicted_bytes > 0);
    assert_eq!(reopened.storage_status().unwrap().cache.resident_bytes, 0);
    reopened.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_reopened_store_exports_a_self_contained_fresh_snapshot() {
    let root = fixture_root("export");
    let index = Index::build(&root, definitions(), RepositoryOptions::default()).unwrap();
    index
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    drop(index);

    let reopened = Index::open_storage(
        &root,
        definitions(),
        RepositoryOptions::default(),
        root.join("storage"),
        StorageOpenOptions::default(),
    )
    .unwrap();
    let export = root.join("export");
    std::fs::create_dir_all(&export).unwrap();
    let report = reopened
        .archive_to(&export, StorageWriteOptions::default())
        .unwrap();
    assert_eq!(report.generation, 1);
    assert!(
        reopened
            .archive_to(root.join("storage"), StorageWriteOptions::default())
            .unwrap()
            .unchanged,
        "exporting a copy must not discard the original store binding"
    );
    reopened.shutdown().unwrap();
    drop(reopened);
    std::fs::rename(root.join("storage"), root.join("storage.offline")).unwrap();

    let exported = Index::open_storage(
        &root,
        definitions(),
        RepositoryOptions::default(),
        &export,
        StorageOpenOptions::default(),
    )
    .unwrap();
    assert_eq!(
        exported
            .view()
            .unwrap()
            .search(&all_records())
            .unwrap()
            .count,
        Some(2)
    );
    let mut fts = all_records();
    fts.fts = Some(FtsQuery {
        case_sensitivity: CaseSensitivity::Sensitive,
        mode: FtsMode::Literal,
        literal: "token".into(),
        pattern_parameter: None,
        fields: Vec::new(),
    });
    assert_eq!(
        exported.view().unwrap().search(&fts).unwrap().count,
        Some(2)
    );
    exported.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn empty_completed_index_round_trips() {
    let root = fixture_root("empty");
    std::fs::remove_file(root.join("data/a.json")).unwrap();
    std::fs::remove_file(root.join("data/b.json")).unwrap();
    let index = Index::build(&root, definitions(), RepositoryOptions::default()).unwrap();
    index
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    drop(index);
    let reopened = Index::open_storage(
        &root,
        definitions(),
        RepositoryOptions::default(),
        root.join("storage"),
        StorageOpenOptions::default(),
    )
    .unwrap();
    assert_eq!(
        reopened
            .view()
            .unwrap()
            .search(&all_records())
            .unwrap()
            .count,
        Some(0)
    );
    reopened.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn reopened_store_rejects_a_different_source_root() {
    let root = fixture_root("source-mismatch");
    let index = Index::build(&root, definitions(), RepositoryOptions::default()).unwrap();
    index
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    drop(index);
    let other_root = root.join("other-source");
    std::fs::create_dir_all(other_root.join("data")).unwrap();
    let error = Index::open_storage(
        &other_root,
        definitions(),
        RepositoryOptions::default(),
        root.join("storage"),
        StorageOpenOptions::default(),
    )
    .err()
    .expect("a different source root must be rejected");
    assert!(error.to_string().contains("source root"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn concurrent_first_use_shares_one_stored_posting_load() {
    let root = fixture_root("concurrent-load");
    let index = Index::build(&root, definitions(), RepositoryOptions::default()).unwrap();
    index
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    drop(index);
    let reopened = std::sync::Arc::new(
        Index::open_storage(
            &root,
            definitions(),
            RepositoryOptions::default(),
            root.join("storage"),
            StorageOpenOptions::default(),
        )
        .unwrap(),
    );
    let cold = reopened.storage_status().unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
    let mut workers = Vec::new();
    for _ in 0..2 {
        let reopened = std::sync::Arc::clone(&reopened);
        let barrier = std::sync::Arc::clone(&barrier);
        workers.push(std::thread::spawn(move || {
            let mut request = all_records();
            request.filter = Some(parse_filter("title = \"Alpha token\"").unwrap());
            barrier.wait();
            reopened.view().unwrap().search(&request).unwrap().count
        }));
    }
    barrier.wait();
    for worker in workers {
        assert_eq!(worker.join().unwrap(), Some(1));
    }
    let warm = reopened.storage_status().unwrap();
    assert_eq!(
        warm.index_reads,
        cold.index_reads + 2,
        "two concurrent accesses must share one dictionary read and one selected leaf read"
    );
    reopened.shutdown().unwrap();
    drop(reopened);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn different_stored_components_can_load_concurrently() {
    let root = fixture_root("concurrent-components");
    let index = Index::build(&root, definitions(), RepositoryOptions::default()).unwrap();
    index
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    drop(index);
    let reopened = std::sync::Arc::new(
        Index::open_storage(
            &root,
            definitions(),
            RepositoryOptions::default(),
            root.join("storage"),
            StorageOpenOptions {
                max_concurrent_reads: std::num::NonZeroUsize::new(2).unwrap(),
                ..StorageOpenOptions::default()
            },
        )
        .unwrap(),
    );
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
    let mut workers = Vec::new();
    for (filter, expected) in [
        ("title = \"Alpha token\"", 1),
        ("title = \"Beta token\"", 1),
    ] {
        let reopened = std::sync::Arc::clone(&reopened);
        let barrier = std::sync::Arc::clone(&barrier);
        workers.push(std::thread::spawn(move || {
            let mut request = all_records();
            request.filter = Some(parse_filter(filter).unwrap());
            barrier.wait();
            (
                reopened.view().unwrap().search(&request).unwrap().count,
                expected,
            )
        }));
    }
    barrier.wait();
    for worker in workers {
        let (actual, expected) = worker.join().unwrap();
        assert_eq!(actual, Some(expected));
    }
    reopened.shutdown().unwrap();
    drop(reopened);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[cfg(unix)]
fn non_utf8_source_paths_round_trip_without_loss() {
    use std::os::unix::ffi::OsStringExt;

    let root = std::env::temp_dir().join(format!(
        "objekte-storage-nonutf-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let path = root
        .join("data")
        .join(std::ffi::OsString::from_vec(b"bad-\xff.json".to_vec()));
    std::fs::write(&path, br#"{"id":"bad","title":"Lossless token"}"#).unwrap();
    let index = Index::build(&root, definitions(), RepositoryOptions::default()).unwrap();
    index
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    drop(index);
    let reopened = Index::open_storage(
        &root,
        definitions(),
        RepositoryOptions::default(),
        root.join("storage"),
        StorageOpenOptions::default(),
    )
    .unwrap();
    assert!(
        reopened
            .view()
            .unwrap()
            .file_handle_for_path(&path)
            .is_some()
    );
    reopened.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn opening_a_missing_store_does_not_initialize_it() {
    let root = fixture_root("missing");
    let missing = root.join("not-a-store");
    assert!(
        Index::open_storage(
            &root,
            definitions(),
            RepositoryOptions::default(),
            &missing,
            StorageOpenOptions::default(),
        )
        .is_err()
    );
    assert!(!missing.exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn save_refuses_an_unrelated_nonempty_directory() {
    let root = fixture_root("nonempty");
    let target = root.join("unrelated");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(target.join("valuable.txt"), b"keep me").unwrap();
    let index = Index::build(&root, definitions(), RepositoryOptions::default()).unwrap();
    let error = index
        .archive_to(&target, StorageWriteOptions::default())
        .expect_err("an unrelated nonempty directory must be rejected");
    assert!(error.to_string().contains("not an Objekte store"));
    assert_eq!(
        std::fs::read(target.join("valuable.txt")).unwrap(),
        b"keep me"
    );
    assert!(!target.join("objects").exists());
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn full_text_payload_reopens_lazily_and_preserves_results() {
    let root = fixture_root("fts");
    let index = Index::build(&root, definitions(), RepositoryOptions::default()).unwrap();
    let mut request = all_records();
    request.options.count = false;
    request.fts = Some(FtsQuery {
        case_sensitivity: CaseSensitivity::Sensitive,
        mode: FtsMode::Literal,
        literal: "token".into(),
        pattern_parameter: None,
        fields: Vec::new(),
    });
    let native = index.view().unwrap().search(&request).unwrap();
    index
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    drop(index);
    let reopened = Index::open_storage(
        &root,
        definitions(),
        RepositoryOptions::default(),
        root.join("storage"),
        StorageOpenOptions::default(),
    )
    .unwrap();
    let cold = reopened.storage_status().unwrap();
    let restored = reopened.view().unwrap().search(&request).unwrap();
    let warm = reopened.storage_status().unwrap();
    assert!(warm.index_reads > cold.index_reads);
    assert!(warm.cache.loaded_families > 0);
    reopened.view().unwrap().search(&request).unwrap();
    assert_eq!(
        reopened.storage_status().unwrap().index_reads,
        warm.index_reads
    );
    assert_eq!(
        restored
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        native
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>()
    );
    reopened.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn trimming_decoded_fts_leaves_allows_a_later_checked_reload() {
    let root = fixture_root("fts-cache-trim");
    let index = Index::build(&root, definitions(), RepositoryOptions::default()).unwrap();
    index
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    drop(index);
    let reopened = Index::open_storage(
        &root,
        definitions(),
        RepositoryOptions::default(),
        root.join("storage"),
        StorageOpenOptions::default(),
    )
    .unwrap();
    let mut request = all_records();
    request.fts = Some(FtsQuery {
        case_sensitivity: CaseSensitivity::Sensitive,
        mode: FtsMode::Literal,
        literal: "token".into(),
        pattern_parameter: None,
        fields: Vec::new(),
    });
    assert_eq!(
        reopened.view().unwrap().search(&request).unwrap().count,
        Some(2)
    );
    let warm = reopened.storage_status().unwrap();
    assert!(warm.cache.resident_bytes > 0);

    let trimmed = reopened.trim_storage_cache().unwrap();
    assert!(trimmed.evicted_entries > 0);
    assert!(trimmed.evicted_bytes > 0);
    assert_eq!(
        reopened.view().unwrap().search(&request).unwrap().count,
        Some(2)
    );
    let reloaded = reopened.storage_status().unwrap();
    assert!(reloaded.index_reads > warm.index_reads);
    reopened.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn missing_stored_fts_gram_skips_all_leaf_reads() {
    let root = fixture_root("fts-missing-gram");
    let index = Index::build(&root, definitions(), RepositoryOptions::default()).unwrap();
    index
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    drop(index);
    let reopened = Index::open_storage(
        &root,
        definitions(),
        RepositoryOptions::default(),
        root.join("storage"),
        StorageOpenOptions::default(),
    )
    .unwrap();
    let cold = reopened.storage_status().unwrap();
    let mut request = all_records();
    request.fts = Some(FtsQuery {
        case_sensitivity: CaseSensitivity::Sensitive,
        mode: FtsMode::Literal,
        literal: "tokenzz".into(),
        pattern_parameter: None,
        fields: Vec::new(),
    });
    let result = reopened.view().unwrap().search(&request).unwrap();
    let warm = reopened.storage_status().unwrap();

    assert_eq!(result.count, Some(0));
    assert!(warm.index_reads > cold.index_reads);
    assert!(
        warm.cache.resident_bytes < 16 * 1024,
        "a missing gram must not load stored sibling leaves: {:?}",
        warm.cache
    );
    reopened.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn relation_payload_loads_only_for_a_relation_query_after_reopen() {
    let root = std::env::temp_dir().join(format!(
        "objekte-storage-relation-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("entries")).unwrap();
    std::fs::create_dir_all(root.join("mentions")).unwrap();
    std::fs::create_dir_all(root.join("storage")).unwrap();
    std::fs::write(root.join("entries/a.json"), br#"{"id":"A","title":"Hymn"}"#).unwrap();
    std::fs::write(
        root.join("entries/b.json"),
        br#"{"id":"B","title":"Festival"}"#,
    )
    .unwrap();
    std::fs::write(
        root.join("mentions/m1.json"),
        br#"{"id":"m1","title":"One","entry":"A"}"#,
    )
    .unwrap();
    std::fs::write(
        root.join("mentions/m2.json"),
        br#"{"id":"m2","title":"Two","entry":"B"}"#,
    )
    .unwrap();
    let definitions = relation_definitions();
    let mut repository =
        Repository::open_with_options(&root, definitions.clone(), RepositoryOptions::default())
            .unwrap();
    let mut request = all_records();
    request.collections = vec!["mentions".into()];
    request.filter = Some(parse_filter("entry.title = 'Hymn'").unwrap());
    let native = repository.search(&request).unwrap();
    assert_eq!(
        native
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["m1"]
    );
    repository
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    drop(repository);

    let reopened = Repository::open_storage(
        &root,
        definitions,
        RepositoryOptions::default(),
        root.join("storage"),
        StorageOpenOptions::default(),
    )
    .unwrap();
    let cold = reopened.storage_status().unwrap();
    let restored = reopened.search(&request).unwrap();
    let warm = reopened.storage_status().unwrap();
    assert!(warm.index_reads > cold.index_reads);
    assert_eq!(
        restored
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["m1"]
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn duplicate_record_ids_stay_collection_scoped_after_reopen() {
    let root = std::env::temp_dir().join(format!(
        "objekte-storage-duplicate-id-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("entries")).unwrap();
    std::fs::create_dir_all(root.join("mentions")).unwrap();
    std::fs::create_dir_all(root.join("storage")).unwrap();
    std::fs::write(
        root.join("entries/same.json"),
        br#"{"id":"same","title":"Entry"}"#,
    )
    .unwrap();
    std::fs::write(
        root.join("mentions/same.json"),
        br#"{"id":"same","title":"Mention","entry":"same"}"#,
    )
    .unwrap();
    let definitions = relation_definitions();
    let index = Index::build(&root, definitions.clone(), RepositoryOptions::default()).unwrap();
    let mut query = all_records();
    query.collections.clear();
    query.filter = Some(parse_filter("id = 'same'").unwrap());
    let expected = index.view().unwrap().search(&query).unwrap();
    assert_eq!(expected.count, Some(2));
    index
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    drop(index);
    let reopened = Index::open_storage(
        &root,
        definitions,
        RepositoryOptions::default(),
        root.join("storage"),
        StorageOpenOptions::default(),
    )
    .unwrap();
    let actual = reopened.view().unwrap().search(&query).unwrap();
    assert_eq!(actual.hits, expected.hits);
    assert_eq!(
        actual
            .hits
            .iter()
            .map(|hit| hit.collection.as_str())
            .collect::<std::collections::BTreeSet<_>>(),
        ["entries", "mentions"].into_iter().collect()
    );
    reopened.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn changed_source_revision_is_rejected_when_payload_materialization_requires_it() {
    let root = fixture_root("stale-locator");
    let index = Index::build(&root, definitions(), RepositoryOptions::default()).unwrap();
    index
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    drop(index);
    let reopened = Index::open_storage(
        &root,
        definitions(),
        RepositoryOptions::default(),
        root.join("storage"),
        StorageOpenOptions::default(),
    )
    .unwrap();
    let mut indexed_only = all_records();
    indexed_only.filter = Some(parse_filter("title = \"Alpha token\"").unwrap());
    assert_eq!(
        reopened
            .view()
            .unwrap()
            .search(&indexed_only)
            .unwrap()
            .count,
        Some(1)
    );
    std::fs::write(
        root.join("data/a.json"),
        br#"{"id":"a","title":"Omega token changed"}"#,
    )
    .unwrap();
    assert_eq!(
        reopened
            .view()
            .unwrap()
            .search(&indexed_only)
            .unwrap()
            .count,
        Some(1),
        "an indexed-only query reflects the saved snapshot"
    );
    let mut materialized = indexed_only;
    materialized.options.return_mode = ReturnMode::Logical;
    assert!(
        reopened.view().unwrap().search(&materialized).is_err(),
        "materialization must reject the stale saved locator"
    );
    reopened.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn old_view_can_lazy_load_after_current_root_replacement_and_index_drop() {
    let root = fixture_root("old-view-lazy");
    let index = Index::build(&root, definitions(), RepositoryOptions::default()).unwrap();
    index
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    drop(index);
    let reopened = Index::open_storage(
        &root,
        definitions(),
        RepositoryOptions::default(),
        root.join("storage"),
        StorageOpenOptions::default(),
    )
    .unwrap();
    let old_view = reopened.view().unwrap();
    std::fs::write(
        root.join("data/c.json"),
        br#"{"id":"c","title":"Gamma token"}"#,
    )
    .unwrap();
    reopened.add_file(&root.join("data/c.json")).unwrap();
    reopened
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    reopened.shutdown().unwrap();
    drop(reopened);

    let mut request = all_records();
    request.fts = Some(FtsQuery {
        case_sensitivity: CaseSensitivity::Sensitive,
        mode: FtsMode::Literal,
        literal: "token".into(),
        pattern_parameter: None,
        fields: Vec::new(),
    });
    assert_eq!(old_view.search(&request).unwrap().count, Some(2));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn transient_missing_lazy_dictionary_can_be_retried() {
    let root = fixture_root("transient-retry");
    let index = Index::build(&root, definitions(), RepositoryOptions::default()).unwrap();
    index
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    drop(index);
    let posting_object = object_with_kind(&root.join("storage"), 3);
    let offline = posting_object.with_extension("offline");
    std::fs::rename(&posting_object, &offline).unwrap();
    let reopened = Index::open_storage(
        &root,
        definitions(),
        RepositoryOptions::default(),
        root.join("storage"),
        StorageOpenOptions::default(),
    )
    .unwrap();
    let mut request = all_records();
    request.filter = Some(parse_filter("title = \"Alpha token\"").unwrap());
    assert!(reopened.view().unwrap().search(&request).is_err());
    std::fs::rename(&offline, &posting_object).unwrap();
    assert_eq!(
        reopened.view().unwrap().search(&request).unwrap().count,
        Some(1),
        "a transient read failure must permit a later retry"
    );
    reopened.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn tiny_cache_budget_bypasses_oversized_selected_payloads() {
    let root = fixture_root("budget-bypass");
    let index = Index::build(&root, definitions(), RepositoryOptions::default()).unwrap();
    index
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    drop(index);
    let reopened = Index::open_storage(
        &root,
        definitions(),
        RepositoryOptions::default(),
        root.join("storage"),
        StorageOpenOptions {
            cache_policy: CachePolicy::Budget {
                resident_bytes: std::num::NonZeroU64::new(1).unwrap(),
            },
            ..StorageOpenOptions::default()
        },
    )
    .unwrap();
    let cold = reopened.storage_status().unwrap();
    assert!(cold.eager_record_rows > 0);
    assert_eq!(cold.cache.resident_bytes, 0);
    let mut request = all_records();
    request.filter = Some(parse_filter("title = \"Alpha token\"").unwrap());
    assert_eq!(
        reopened.view().unwrap().search(&request).unwrap().count,
        Some(1)
    );
    let warm = reopened.storage_status().unwrap();
    assert!(warm.cache.oversize_bypasses > 0);
    assert_eq!(warm.cache.resident_bytes, 0);
    assert_eq!(reopened.trim_storage_cache().unwrap().evicted_entries, 0);
    reopened.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn deterministic_128_mutation_sequence_matches_fresh_build_after_checkpoints() {
    let root = fixture_root("mutation-sequence");
    std::fs::remove_file(root.join("data/a.json")).unwrap();
    std::fs::remove_file(root.join("data/b.json")).unwrap();
    let definitions = sequence_definitions();
    let state_path = root.join("data/state.jsonl");
    let mut state = vec![(0, "Base".to_owned())];
    let write_state = |state: &[(usize, String)]| {
        let body = state
            .iter()
            .map(|(id, title)| format!(r#"{{"id":"state-{id}","title":"{title}"}}"#))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        std::fs::write(&state_path, body).unwrap();
    };
    write_state(&state);
    let movable_path = root.join("data/movable.jsonl");
    std::fs::write(&movable_path, br#"{"id":"movable","title":"Move token"}"#).unwrap();

    let index = Index::build(&root, definitions.clone(), RepositoryOptions::default()).unwrap();
    index
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    drop(index);
    let mut reopened = Index::open_storage(
        &root,
        definitions.clone(),
        RepositoryOptions::default(),
        root.join("storage"),
        StorageOpenOptions::default(),
    )
    .unwrap();
    let view = reopened.view().unwrap();
    let mut movable_path = view
        .file_paths()
        .find(|path| path.ends_with("movable.jsonl"))
        .map(|path| {
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                root.join(path)
            }
        })
        .unwrap();
    let mut movable_handle = view.file_handle_for_path(&movable_path).unwrap();
    drop(view);

    for step in 0..32 {
        let add_path = root.join(format!("data/add-{step}.jsonl"));
        std::fs::write(
            &add_path,
            format!(r#"{{"id":"add-{step}","title":"Added {step} token"}}"#),
        )
        .unwrap();
        let added = reopened.add_file(&add_path).unwrap();

        state.push((step + 1, format!("State {step} token")));
        write_state(&state);
        let state_handle = reopened
            .view()
            .unwrap()
            .file_handle_for_path(&state_path)
            .unwrap();
        reopened.update_file(state_handle, &state_path).unwrap();

        let next_movable = if movable_path.ends_with("movable.jsonl") {
            root.join("data/moved.jsonl")
        } else {
            root.join("data/movable.jsonl")
        };
        std::fs::rename(&movable_path, &next_movable).unwrap();
        movable_handle = reopened
            .move_file(movable_handle, &next_movable)
            .unwrap()
            .file;
        movable_path = next_movable;

        reopened.delete_file(added.file).unwrap();
        std::fs::remove_file(&add_path).unwrap();

        if (step + 1) % 8 == 0 {
            reopened
                .archive_to(root.join("storage"), StorageWriteOptions::default())
                .unwrap();
            reopened.shutdown().unwrap();
            drop(reopened);
            reopened = Index::open_storage(
                &root,
                definitions.clone(),
                RepositoryOptions::default(),
                root.join("storage"),
                StorageOpenOptions::default(),
            )
            .unwrap();
            let view = reopened.view().unwrap();
            movable_path = view
                .file_paths()
                .find(|path| {
                    path.file_name().is_some_and(|name| {
                        matches!(name.to_str(), Some("movable.jsonl" | "moved.jsonl"))
                    })
                })
                .map(|path| {
                    if path.is_absolute() {
                        path.to_path_buf()
                    } else {
                        root.join(path)
                    }
                })
                .unwrap();
            movable_handle = view.file_handle_for_path(&movable_path).unwrap();
            drop(view);

            let fresh =
                Index::build(&root, definitions.clone(), RepositoryOptions::default()).unwrap();
            let mut sorted = all_records();
            sorted.options.sort = vec![SortField::simple("title", false)];
            let expected = fresh.view().unwrap().search(&sorted).unwrap();
            let actual = reopened.view().unwrap().search(&sorted).unwrap();
            assert_eq!(actual.count, expected.count, "checkpoint {}", step + 1);
            assert_eq!(
                actual
                    .hits
                    .iter()
                    .map(|hit| (hit.collection.as_str(), hit.record_id.as_str()))
                    .collect::<Vec<_>>(),
                expected
                    .hits
                    .iter()
                    .map(|hit| (hit.collection.as_str(), hit.record_id.as_str()))
                    .collect::<Vec<_>>(),
                "checkpoint {}",
                step + 1
            );
        }
    }
    reopened.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn first_save_after_delete_preserves_a_dense_file_domain() {
    let root = fixture_root("first-save-delete");
    let index = Index::build(&root, definitions(), RepositoryOptions::default()).unwrap();
    let path = root.join("data/c.json");
    std::fs::write(&path, br#"{"id":"c","title":"Gamma token"}"#).unwrap();
    let added = index.add_file(&path).unwrap();
    index.delete_file(added.file).unwrap();
    std::fs::remove_file(path).unwrap();
    index
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    drop(index);
    let reopened = Index::open_storage(
        &root,
        definitions(),
        RepositoryOptions::default(),
        root.join("storage"),
        StorageOpenOptions::default(),
    )
    .unwrap();
    assert_eq!(
        reopened
            .view()
            .unwrap()
            .search(&all_records())
            .unwrap()
            .count,
        Some(2)
    );
    reopened.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn zero_one_and_many_record_file_associations_round_trip() {
    let root = fixture_root("file-cardinalities");
    std::fs::remove_file(root.join("data/a.json")).unwrap();
    std::fs::remove_file(root.join("data/b.json")).unwrap();
    std::fs::write(root.join("data/zero.jsonl"), "").unwrap();
    std::fs::write(
        root.join("data/one.jsonl"),
        "{\"id\":\"one\",\"title\":\"One token\"}\n",
    )
    .unwrap();
    std::fs::write(
        root.join("data/many.jsonl"),
        "{\"id\":\"many-a\",\"title\":\"Many A token\"}\n{\"id\":\"many-b\",\"title\":\"Many B token\"}\n",
    )
    .unwrap();
    let definitions = sequence_definitions();
    let index = Index::build(&root, definitions.clone(), RepositoryOptions::default()).unwrap();
    index
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    drop(index);
    let reopened = Index::open_storage(
        &root,
        definitions,
        RepositoryOptions::default(),
        root.join("storage"),
        StorageOpenOptions::default(),
    )
    .unwrap();
    let view = reopened.view().unwrap();
    assert_eq!(view.file_paths().count(), 3);
    let result = view.search(&all_records()).unwrap();
    assert_eq!(result.count, Some(3));
    assert_eq!(
        result
            .hits
            .iter()
            .filter(|hit| hit.file_path.ends_with("many.jsonl"))
            .count(),
        2
    );
    assert_eq!(
        result
            .hits
            .iter()
            .filter(|hit| hit.file_path.ends_with("one.jsonl"))
            .count(),
        1
    );
    drop(view);
    reopened.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn normal_skip_and_raw_fallback_fts_match_after_reopen() {
    let root = fixture_root("fts-modes");
    std::fs::write(
        root.join("data/a.json"),
        br#"{"id":"a","title":"Supercalifragilisticexpialidocious"}"#,
    )
    .unwrap();
    std::fs::write(root.join("data/b.json"), b"invalid json fallbackword bytes").unwrap();
    std::fs::write(root.join("data/c.json"), r#"{"id":"c","title":"Straße"}"#).unwrap();
    let index = Index::build(&root, definitions(), RepositoryOptions::default()).unwrap();
    let cases = [
        ("token", CaseSensitivity::Sensitive),
        ("fragilistic", CaseSensitivity::Sensitive),
        ("fallbackword", CaseSensitivity::Sensitive),
        ("strasse", CaseSensitivity::Insensitive),
    ];
    let run_case = |view: &objekte_search_core::lifecycle::IndexView| {
        cases
            .iter()
            .map(|(literal, case_sensitivity)| {
                let mut request = all_records();
                request.fts = Some(FtsQuery {
                    case_sensitivity: *case_sensitivity,
                    mode: FtsMode::Literal,
                    literal: (*literal).into(),
                    pattern_parameter: None,
                    fields: Vec::new(),
                });
                view.search(&request).unwrap()
            })
            .collect::<Vec<_>>()
    };
    let native = run_case(&index.view().unwrap());
    index
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    drop(index);
    let reopened = Index::open_storage(
        &root,
        definitions(),
        RepositoryOptions::default(),
        root.join("storage"),
        StorageOpenOptions::default(),
    )
    .unwrap();
    let restored = run_case(&reopened.view().unwrap());
    for (actual, expected) in restored.into_iter().zip(native) {
        assert_eq!(actual.count, expected.count);
        assert_eq!(actual.hits, expected.hits);
    }
    reopened.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn empty_fts_intersection_stops_before_loading_every_gram_leaf() {
    let root = fixture_root("fts-empty-intersection");
    for (name, title) in [("a", "abcdef"), ("b", "efgh"), ("c", "ghijkl")] {
        std::fs::write(
            root.join(format!("data/{name}.json")),
            format!(r#"{{"id":"{name}","title":"{title}"}}"#),
        )
        .unwrap();
    }
    let index = Index::build(&root, definitions(), RepositoryOptions::default()).unwrap();
    index
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    drop(index);
    let reopened = Index::open_storage(
        &root,
        definitions(),
        RepositoryOptions::default(),
        root.join("storage"),
        StorageOpenOptions::default(),
    )
    .unwrap();
    let cold = reopened.storage_status().unwrap();
    let mut request = all_records();
    request.fts = Some(FtsQuery {
        case_sensitivity: CaseSensitivity::Sensitive,
        mode: FtsMode::Literal,
        literal: "abcdefgh".into(),
        pattern_parameter: None,
        fields: Vec::new(),
    });
    assert_eq!(
        reopened.view().unwrap().search(&request).unwrap().count,
        Some(0)
    );
    let warm = reopened.storage_status().unwrap();
    assert!(warm.index_reads < cold.index_reads + 9);
    assert!(warm.cache.loaded_families < 7);
    reopened.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn pack_handle_cache_stays_bounded_while_selected_columns_load() {
    let root = fixture_root("handle-cache");
    std::fs::remove_file(root.join("data/a.json")).unwrap();
    std::fs::remove_file(root.join("data/b.json")).unwrap();
    let mut definition = CollectionDefinition::folder("items", "data", SourceFormat::Json);
    definition.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
    let mut document = serde_json::Map::new();
    document.insert("id".into(), serde_json::Value::String("a".into()));
    definition
        .schema
        .register_field(FieldDefinition {
            path: vec!["id".into()],
            kind: LogicalType::Id,
            required: true,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    for number in 0..70 {
        let name = format!("f{number}");
        document.insert(
            name.clone(),
            serde_json::Value::String(format!("value {number}")),
        );
        definition
            .schema
            .register_field(FieldDefinition {
                path: vec![name.clone()],
                kind: LogicalType::String,
                required: false,
                nullable: false,
                indexed: true,
                multi: false,
                element_nullable: Vec::new(),
            })
            .unwrap();
        definition
            .sort_fields
            .push(objekte_search_core::model::FieldPath::parse(&name).unwrap());
    }
    std::fs::write(
        root.join("data/a.json"),
        serde_json::to_vec(&document).unwrap(),
    )
    .unwrap();
    let definitions = vec![definition];
    let index = Index::build(&root, definitions.clone(), RepositoryOptions::default()).unwrap();
    index
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    drop(index);
    let reopened = Index::open_storage(
        &root,
        definitions,
        RepositoryOptions::default(),
        root.join("storage"),
        StorageOpenOptions::default(),
    )
    .unwrap();
    for number in 0..70 {
        let mut request = all_records();
        request.options.sort = vec![SortField::simple(format!("f{number}"), false)];
        request.options.count = false;
        assert_eq!(
            reopened
                .view()
                .unwrap()
                .search(&request)
                .unwrap()
                .hits
                .len(),
            1
        );
    }
    let status = reopened.storage_status().unwrap();
    assert!(status.open_pack_handles > 0);
    assert!(status.open_pack_handles <= 64);
    reopened.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
