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
    std::fs::write(
        root.join("entries/a.json"),
        br#"{"id":"A","title":"Hymn"}"#,
    )
    .unwrap();
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
