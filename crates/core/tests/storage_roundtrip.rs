use objekte_search_core::lifecycle::Index;
use objekte_search_core::lifecycle::{CompactionMode, CompactionOptions};
use objekte_search_core::model::LogicalType;
use objekte_search_core::query::{
    CaseSensitivity, FtsMode, FtsQuery, Query, QueryOptions, parse_filter,
};
use objekte_search_core::reference::FieldDefinition;
use objekte_search_core::repository::{CollectionDefinition, RepositoryOptions};
use objekte_search_core::source::SourceFormat;
use objekte_search_core::storage::{
    AuditOptions, StorageOpenOptions, StorageReadLimits, StorageWriteOptions, audit, prune,
};
use objekte_search_core::workload::{ReturnMode, SortField};

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

#[test]
fn explicit_save_reopens_a_complete_searchable_snapshot() {
    let root = fixture_root("roundtrip");
    let index = Index::build(
        &root,
        definitions(),
        RepositoryOptions::default(),
    )
    .unwrap();
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
    let index = Index::build(
        &root,
        definitions(),
        RepositoryOptions::default(),
    )
    .unwrap();
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

    reopened.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn configured_sort_frames_load_only_when_sort_execution_needs_them() {
    let root = fixture_root("sort-lazy");
    let index = Index::build(
        &root,
        definitions(),
        RepositoryOptions::default(),
    )
    .unwrap();
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
fn add_and_delete_after_reopen_publish_a_new_storage_generation() {
    let root = fixture_root("mutation");
    let index = Index::build(
        &root,
        definitions(),
        RepositoryOptions::default(),
    )
    .unwrap();
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
        reopened.view().unwrap().search(&all_records()).unwrap().count,
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
        reopened.view().unwrap().search(&all_records()).unwrap().count,
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
    let index = Index::build(
        &root,
        definitions(),
        RepositoryOptions::default(),
    )
    .unwrap();
    index
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    drop(index);

    let posting_object = object_with_kind(&root.join("storage"), 3);
    let mut bytes = std::fs::read(&posting_object).unwrap();
    bytes.truncate(bytes.len() - 1);
    std::fs::write(&posting_object, bytes).unwrap();
    let orphan = root.join(format!(
        "storage/objects/{}.pack",
        "a".repeat(64)
    ));
    std::fs::write(&orphan, b"unreferenced").unwrap();
    let unrelated = root.join("storage/objects/README");
    std::fs::write(&unrelated, b"not owned by the backend").unwrap();

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

    let audit_report = audit(
        &root.join("storage"),
        AuditOptions::default(),
    )
    .unwrap();
    assert!(!audit_report.errors.is_empty());
    assert!(prune(&root.join("storage")).is_err());
    reopened.shutdown().unwrap();
    drop(reopened);

    let removed = prune(&root.join("storage")).unwrap();
    assert!(removed.removed_objects >= 1);
    assert!(!orphan.exists());
    assert!(unrelated.exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn full_compaction_after_reopen_does_not_parse_source_files() {
    let root = fixture_root("source-free-compaction");
    let index = Index::build(
        &root,
        definitions(),
        RepositoryOptions::default(),
    )
    .unwrap();
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
        reopened.view().unwrap().search(&all_records()).unwrap().count,
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
    let index = Index::build(
        &root,
        definitions(),
        RepositoryOptions::default(),
    )
    .unwrap();
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
    replacement_query.filter =
        Some(parse_filter("title = \"Omega replacement\"").unwrap());
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
    let first = Index::build(
        &root,
        definitions(),
        RepositoryOptions::default(),
    )
    .unwrap();
    first
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    drop(first);

    let second = Index::build(
        &root,
        definitions(),
        RepositoryOptions::default(),
    )
    .unwrap();
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
    let index = Index::build(
        &root,
        definitions(),
        RepositoryOptions::default(),
    )
    .unwrap();
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
    exported.shutdown().unwrap();
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
    assert_eq!(warm.index_reads, cold.index_reads + 2);
    reopened.shutdown().unwrap();
    drop(reopened);
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
    let restored = reopened.view().unwrap().search(&request).unwrap();
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




