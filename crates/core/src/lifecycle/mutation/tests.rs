use super::{
    FileMutationKind, FileMutationStrategy, Index, IndexError, UpdateOptions, UpdateStrategy,
    VerifiedFallbackReason,
};
use crate::inventory::SourceRevision;
use crate::lifecycle::mutation::DeleteStrategy;
use crate::model::{CollectionId, FieldPath, LogicalType, RecordSlot};
use crate::query::options::{ReturnMode, SortField};
use crate::query::{CaseSensitivity, FtsMode, FtsQuery, Query, QueryOptions, parse_filter};
use crate::repository::configuration::{CollectionDefinition, IdentityPolicy, RepositoryOptions};
use crate::schema::relations::{RelationCardinality, RelationDefinition};
use crate::schema::{FieldDefinition, Schema};
use crate::source::SourceFormat;

fn test_root(label: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!(
        "objekte-mutation-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    root
}

fn index(root: &std::path::Path) -> Index {
    crate::lifecycle::Index::start_build(
        root,
        vec![CollectionDefinition::folder(
            "items",
            "data",
            SourceFormat::JsonLines,
        )],
        RepositoryOptions::default(),
        crate::source::registry::SourceRegistry::builtin(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap()
}

fn intrinsic_definition(name: &str) -> CollectionDefinition {
    let mut definition = CollectionDefinition::folder(name, "data", SourceFormat::JsonLines);
    definition.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
    definition
        .schema
        .register_field(FieldDefinition {
            path: vec!["id".into()],
            kind: LogicalType::Id,
            required: true,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    definition
}

#[test]
fn deleting_obsolete_file_incarnation_keeps_new_owner_route() {
    let root = test_root("obsolete-file-incarnation");
    let index = crate::lifecycle::Index::start_build(
        &root,
        vec![intrinsic_definition("items")],
        RepositoryOptions::default(),
        crate::source::registry::SourceRegistry::builtin(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let a = root.join("data/a.jsonl");
    let b = root.join("data/b.jsonl");
    let c = root.join("data/c.jsonl");
    std::fs::write(&a, b"{\"id\":\"K\"}\n").unwrap();
    let a_handle = index.add_file(&a).unwrap().file;
    std::fs::write(&a, b"{\"id\":\"J\",\"note\":\"replacement\"}\n").unwrap();
    let a_handle = index.update_file(a_handle, &a).unwrap().file;
    std::fs::write(&b, b"{\"id\":\"K\",\"note\":\"new owner\"}\n").unwrap();
    let b_handle = index.add_file(&b).unwrap().file;
    let pinned = index.view().unwrap();
    assert_eq!(
        pinned
            .file_catalog()
            .unwrap()
            .file_for_record("items", "K")
            .unwrap()
            .unwrap()
            .file_id,
        b_handle.file_id
    );

    index.delete_file(a_handle).unwrap();
    let view = index.view().unwrap();
    let hit = view
        .get_record("items", "K", ReturnMode::Logical)
        .unwrap()
        .unwrap();
    assert_eq!(hit.record_id, "K");
    assert_eq!(
        view.file_catalog()
            .unwrap()
            .file_for_record("items", "K")
            .unwrap()
            .unwrap()
            .file_id,
        b_handle.file_id
    );
    assert_eq!(
        view.file_catalog()
            .unwrap()
            .records_for_file(b_handle.file_id)
            .unwrap()
            .len(),
        1
    );
    let query = Query {
        collections: vec!["items".into()],
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
    assert_eq!(view.search(&query).unwrap().count, Some(1));
    assert_eq!(
        pinned
            .get_record("items", "J", ReturnMode::Locator)
            .unwrap()
            .unwrap()
            .record_id,
        "J"
    );
    assert_eq!(
        pinned
            .get_record("items", "K", ReturnMode::Locator)
            .unwrap()
            .unwrap()
            .record_id,
        "K"
    );
    std::fs::write(&c, b"{\"id\":\"K\"}\n").unwrap();
    assert!(index.add_file(&c).is_err());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn pinned_views_keep_membership_and_file_handles_across_same_file_replacement() {
    let root = test_root("pinned-same-file-replacement");
    let path = root.join("data/a.jsonl");
    std::fs::write(&path, b"{\"id\":\"a\",\"rank\":10}\n").unwrap();
    let mut definition = intrinsic_definition("items");
    definition
        .schema
        .register_field(FieldDefinition {
            path: vec!["rank".into()],
            kind: LogicalType::Number,
            required: true,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    let index = crate::lifecycle::Index::start_build(
        &root,
        vec![definition],
        RepositoryOptions {
            verified_cache_bytes: 0,
            ..Default::default()
        },
        crate::source::registry::SourceRegistry::builtin(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let old_view = index.view().unwrap();
    let old_revision = old_view.revision();
    let old_handle = old_view.file_handle_for_path(&path).unwrap();
    let query = |filter: &str| Query {
        collections: vec!["items".into()],
        fts: None,
        filter: Some(parse_filter(filter).unwrap()),
        options: QueryOptions {
            sort: Vec::new(),
            limit: 10,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    assert_eq!(old_view.count_exact(&query("rank = 10")).unwrap(), 1);
    assert_eq!(old_view.count_exact(&query("rank = 2002")).unwrap(), 0);

    std::fs::write(&path, b"{\"id\":\"a\",\"rank\":2002}\n").unwrap();
    let updated = index.update_file(old_handle, &path).unwrap();
    let new_view = index.view().unwrap();
    assert_eq!(updated.revision, old_revision.next().unwrap());
    assert_eq!(old_view.revision(), old_revision);
    assert_eq!(new_view.revision(), updated.revision);
    assert_eq!(old_view.count_exact(&query("rank = 10")).unwrap(), 1);
    assert_eq!(old_view.count_exact(&query("rank = 2002")).unwrap(), 0);
    assert_eq!(new_view.count_exact(&query("rank = 10")).unwrap(), 0);
    assert_eq!(new_view.count_exact(&query("rank = 2002")).unwrap(), 1);
    assert_eq!(old_view.file_handle_for_path(&path).unwrap(), old_handle);
    assert_eq!(new_view.file_handle_for_path(&path).unwrap(), updated.file);
    assert_ne!(updated.file, old_handle);

    drop(new_view);
    drop(old_view);
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn inferred_order_uses_the_current_snapshot_after_multi_segment_mutation() {
    use crate::inference::{InferenceConfig, InferenceMode};

    let root = test_root("inferred-current-snapshot");
    let first = root.join("data/first.jsonl");
    let second = root.join("data/second.jsonl");
    std::fs::write(&first, b"{\"id\":\"a\",\"year\":20}\n").unwrap();
    std::fs::write(&second, b"{\"id\":\"b\",\"year\":30}\n").unwrap();
    let mut definition = intrinsic_definition("items");
    definition
        .schema
        .register_field(FieldDefinition {
            path: vec!["year".into()],
            kind: LogicalType::Number,
            required: false,
            nullable: true,
            indexed: false,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    let options = RepositoryOptions {
        segment_policy: crate::index::segment::SegmentPolicy {
            target_records: std::num::NonZeroUsize::new(1).unwrap(),
            max_files: std::num::NonZeroUsize::new(1).unwrap(),
        },
        inference: InferenceConfig {
            mode: InferenceMode::AutoIndex,
            ..Default::default()
        },
        ..Default::default()
    };
    let index = crate::lifecycle::Index::start_build(
        &root,
        vec![definition],
        options,
        crate::source::registry::SourceRegistry::builtin(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let third = root.join("data/third.jsonl");
    std::fs::write(&third, b"{\"id\":\"c\",\"year\":10}\n").unwrap();
    let added = index.add_file(&third).unwrap();
    let request = |descending: bool, offset: usize, count: bool| Query {
        collections: vec!["items".into()],
        fts: None,
        filter: None,
        options: QueryOptions {
            sort: vec![SortField::simple("year", descending)],
            limit: 1,
            offset,
            count,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    for (request, expected) in [
        (request(false, 0, true), "c"),
        (request(true, 0, true), "b"),
        (request(false, 1, true), "a"),
    ] {
        let view = index.view().unwrap();
        let result = view.search(&request).unwrap();
        let reference = view.search_reference(&request).unwrap();
        assert_eq!(result.hits, reference.hits);
        assert_eq!(result.count, reference.count);
        assert_eq!(result.hits[0].record_id, expected);
        assert_eq!(result.counters.files_opened, 0);
        assert_eq!(result.counters.record_parses, 0);
    }

    let first_handle = index.view().unwrap().file_handle_for_path(&first).unwrap();
    index.delete_file(first_handle).unwrap();
    index.delete_file(added.file).unwrap();
    let request = request(false, 0, true);
    let view = index.view().unwrap();
    let result = view.search(&request).unwrap();
    let reference = view.search_reference(&request).unwrap();
    assert_eq!(result.hits, reference.hits);
    assert_eq!(result.count, reference.count);
    assert_eq!(result.hits[0].record_id, "b");
    assert_eq!(result.counters.files_opened, 0);

    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn selected_null_and_presence_survive_explicit_mutations_and_compaction() {
    let root = test_root("structural-postings");
    let first = root.join("data/first.json");
    std::fs::write(&first, r#"{"tags":[],"text":""}"#).unwrap();
    let mut definition = CollectionDefinition::folder("items", "data", SourceFormat::Json);
    for (name, kind, multi) in [
        (
            "tags",
            LogicalType::Array(Box::new(LogicalType::Object)),
            true,
        ),
        ("text", LogicalType::String, false),
    ] {
        definition
            .schema
            .register_field(FieldDefinition {
                path: vec![name.into()],
                kind,
                required: false,
                nullable: false,
                indexed: false,
                multi,
                element_nullable: vec![],
            })
            .unwrap();
    }
    let options = RepositoryOptions {
        verified_cache_bytes: 0,
        ..Default::default()
    };
    let index = crate::lifecycle::Index::start_build(
        &root,
        vec![definition.clone()],
        options,
        crate::source::registry::SourceRegistry::builtin(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let check = |index: &Index, expected: [usize; 3], compare_reference: bool| {
        let view = index.view().unwrap();
        for (filter, count) in ["tags:length = 0", "tags:length = 2", "text = \"\""]
            .into_iter()
            .zip(expected)
        {
            let request = Query {
                collections: vec!["items".into()],
                fts: None,
                filter: Some(parse_filter(filter).unwrap()),
                options: QueryOptions {
                    sort: vec![],
                    limit: 50,
                    offset: 0,
                    count: true,
                    return_mode: ReturnMode::Locator,
                    evidence: true,
                },
            };
            let actual = view.search(&request).unwrap();
            assert_eq!(actual.count, Some(count), "{filter}");
            if compare_reference {
                let reference = view.search_reference(&request).unwrap();
                assert_eq!(actual.count, reference.count, "{filter}");
                assert_eq!(actual.hits, reference.hits, "{filter}");
            }
            assert_eq!(actual.counters.files_opened, 0, "{filter}");
            assert_eq!(actual.counters.bytes_read, 0, "{filter}");
            assert_eq!(actual.counters.record_parses, 0, "{filter}");
        }
    };
    check(&index, [1, 0, 1], true);
    let first_handle = index.view().unwrap().file_handle_for_path(&first).unwrap();
    std::fs::write(&first, r#"{"tags":[{},{}],"text":"filled"}"#).unwrap();
    // Search remains bound to the published facts until the explicit update.
    check(&index, [1, 0, 1], false);
    let updated = index.update_file(first_handle, &first).unwrap();
    check(&index, [0, 1, 0], true);
    let second = root.join("data/second.json");
    std::fs::write(&second, r#"{"tags":[],"text":""}"#).unwrap();
    let added = index.add_file(&second).unwrap();
    check(&index, [1, 1, 1], true);
    let moved = root.join("data/moved.json");
    std::fs::rename(&first, &moved).unwrap();
    index.move_file(updated.file, &moved).unwrap();
    check(&index, [1, 1, 1], true);
    std::fs::remove_file(&second).unwrap();
    index.delete_file(added.file).unwrap();
    check(&index, [0, 1, 0], true);
    index.compact().unwrap();
    check(&index, [0, 1, 0], true);
    check(&index, [0, 1, 0], true);
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn search_trusts_recorded_json_validation_until_explicit_update() {
    use crate::inventory::RecordClass;
    use std::time::{Duration, UNIX_EPOCH};

    let root = test_root("recorded-validation");
    let path = root.join("data/item.json");
    std::fs::write(&path, b"{\"x\":2,\"junk\":0}").unwrap();
    let index = crate::lifecycle::Index::start_build(
        &root,
        vec![CollectionDefinition::folder(
            "items",
            "data",
            SourceFormat::JsonDecodedScalars,
        )],
        RepositoryOptions::default(),
        crate::source::registry::SourceRegistry::builtin(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let mut handle = index.view().unwrap().file_handle_for_path(&path).unwrap();
    let mut query = Query {
        collections: vec!["items".into()],
        fts: None,
        filter: Some(parse_filter("x = 2").unwrap()),
        options: QueryOptions {
            sort: Vec::new(),
            limit: 10,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    let initial = index.view().unwrap().search(&query).unwrap();
    assert_eq!(initial.count, Some(1));
    let record_id = initial.hits[0].record_id.clone();

    // The indexed field remains readable while an unrelated source value is
    // malformed. Search neither reparses that field nor updates classification.
    std::fs::write(&path, b"{\"x\":2,\"junk\":!}").unwrap();
    std::fs::File::open(&path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(10)))
        .unwrap();
    // The pinned revision cannot supply bytes after an external edit.
    assert!(index.view().unwrap().search(&query).is_err());
    query.filter = None;
    assert!(
        index
            .view()
            .unwrap()
            .get_record("items", &record_id, ReturnMode::Bytes)
            .is_err()
    );

    let changed = index.update_file(handle, &path).unwrap();
    handle = changed.file;
    assert_eq!(
        index.view().unwrap().search(&query).unwrap().hits[0].record_class,
        RecordClass::RawFallback
    );
    query.filter = Some(parse_filter("x = 2").unwrap());
    assert_eq!(index.view().unwrap().search(&query).unwrap().count, Some(0));

    std::fs::write(&path, b"{\"x\":2,\"junk\":0}").unwrap();
    std::fs::File::open(&path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(11)))
        .unwrap();
    assert_eq!(index.view().unwrap().search(&query).unwrap().count, Some(0));
    index.update_file(handle, &path).unwrap();
    assert_eq!(index.view().unwrap().search(&query).unwrap().count, Some(1));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn final_mutated_view_matches_independent_fresh_build() {
    let root = test_root("fresh-oracle");
    let first = root.join("data/first.jsonl");
    let second = root.join("data/second.jsonl");
    std::fs::write(&first, b"{\"name\":\"old-a\"}\n{\"name\":\"old-b\"}\n").unwrap();
    std::fs::write(&second, b"{\"name\":\"second\"}\n").unwrap();
    let live = index(&root);
    let original = live.view().unwrap();
    let first_handle = original.file_handle_for_path(&first).unwrap();
    let second_handle = original.file_handle_for_path(&second).unwrap();
    let third = root.join("data/third.jsonl");
    std::fs::write(&third, b"{\"name\":\"third\"}\n").unwrap();
    let third_handle = live.add_file(&third).unwrap().file;
    std::fs::write(&first, b"{\"name\":\"new-a\"}\n").unwrap();
    live.update_file(first_handle, &first).unwrap();
    let moved = root.join("data/moved.jsonl");
    std::fs::rename(&second, &moved).unwrap();
    live.move_file(second_handle, &moved).unwrap();
    std::fs::remove_file(&third).unwrap();
    live.delete_file(third_handle).unwrap();
    live.compact().unwrap();

    let fresh = index(&root);
    let query = Query {
        collections: vec!["items".into()],
        fts: None,
        filter: None,
        options: QueryOptions {
            sort: Vec::new(),
            limit: 100,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Bytes,
            evidence: false,
        },
    };
    let actual = live.view().unwrap().search(&query).unwrap();
    let expected = fresh.view().unwrap().search(&query).unwrap();
    assert_eq!(actual.count, expected.count);
    let rows = |result: crate::query_results::SearchResult| {
        result
            .hits
            .into_iter()
            .map(|hit| {
                (
                    hit.collection,
                    hit.record_id,
                    hit.file.as_ref().expect("file origin").path.clone(),
                    hit.file.as_ref().expect("file origin").status,
                    hit.payload,
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(rows(actual), rows(expected));
    live.shutdown().unwrap();
    fresh.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn public_mutations_add_update_move_and_preserve_version_conflicts() {
    let root = test_root("path");
    let index = index(&root);
    let path = root.join("data/first.jsonl");
    std::fs::write(&path, b"{\"name\":\"one\"}\n").unwrap();
    let added = index.add_file(&path).unwrap();
    assert_eq!(added.kind, FileMutationKind::Added);
    assert_eq!(added.retired_records, 0);
    assert_eq!(added.indexed_records, 1);
    assert_eq!(added.affected_collections, ["items"]);
    assert_eq!(index.status().unwrap().live_records, 1);
    assert!(matches!(
        index.add_file(&path),
        Err(IndexError::DestinationExists)
    ));

    let same = index.update_file(added.file, &path).unwrap();
    assert_eq!(same.kind, FileMutationKind::Unchanged);
    assert_eq!(same.file, added.file);
    assert_eq!(same.revision, added.revision);

    std::fs::write(&path, b"{\"name\":\"two\"}\n{\"name\":\"three\"}\n").unwrap();
    let changed = index.update_file(added.file, &path).unwrap();
    assert_eq!(changed.kind, FileMutationKind::Replaced);
    assert_eq!(changed.retired_records, 1);
    assert_eq!(changed.indexed_records, 2);
    assert_eq!(changed.file.file_id, added.file.file_id);
    assert_ne!(changed.file.version, added.file.version);
    assert_eq!(index.status().unwrap().live_records, 2);
    assert!(matches!(
        index.update_file(added.file, &path),
        Err(IndexError::FileConflict { .. })
    ));

    let destination = root.join("data/moved.jsonl");
    std::fs::rename(&path, &destination).unwrap();
    let moved = index.move_file(changed.file, &destination).unwrap();
    assert_eq!(moved.kind, FileMutationKind::Moved);
    assert_eq!(moved.strategy, FileMutationStrategy::ContentPreservingMove);
    assert_eq!(moved.retired_records, 0);
    assert_eq!(moved.indexed_records, 2);
    assert_eq!(moved.file.file_id, changed.file.file_id);
    assert_ne!(moved.file.version, changed.file.version);
    assert!(index.view().unwrap().file_handle_for_path(&path).is_none());
    assert_eq!(
        index.view().unwrap().file_handle_for_path(&destination),
        Some(moved.file)
    );
    assert!(!path.exists());
    assert!(destination.exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn compatible_move_keeps_index_contributions_and_advances_file_version() {
    let root = test_root("compatible-move");
    let original = root.join("data/original.jsonl");
    std::fs::write(&original, b"{\"name\":\"one\"}\n{\"name\":\"two\"}\n").unwrap();
    let index = index(&root);
    let before = index.view().unwrap();
    let handle = before.file_handle_for_path(&original).unwrap();
    let fingerprint = before.file_fingerprint(handle.file_id);
    let prior_segments = before.0.segments.clone();
    let prior_repository = std::sync::Arc::clone(&before.0.repository);
    let destination = root.join("data/renamed.jsonl");
    std::fs::rename(&original, &destination).unwrap();
    let moved = index.move_file(handle, &destination).unwrap();
    assert_eq!(moved.kind, FileMutationKind::Moved);
    assert_eq!(moved.strategy, FileMutationStrategy::ContentPreservingMove);
    assert_eq!(moved.retired_records, 0);
    assert_eq!(moved.indexed_records, 2);
    assert_ne!(moved.file.version, handle.version);
    let after = index.view().unwrap();
    assert!(after.file_handle_for_path(&original).is_none());
    assert_eq!(after.file_handle_for_path(&destination), Some(moved.file));
    assert_eq!(after.file_fingerprint(handle.file_id), fingerprint);
    assert!(std::sync::Arc::ptr_eq(
        &prior_repository,
        &after.0.repository
    ));
    for (old, new) in prior_segments.iter().zip(&after.0.segments) {
        assert!(std::sync::Arc::ptr_eq(old, new));
    }
    assert!(matches!(
        index.move_file(handle, &original),
        Err(IndexError::FileConflict { .. })
    ));
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn move_hashes_equal_size_equal_mtime_content_and_rejects_missing_destination() {
    use crate::query_results::Payload;

    let root = test_root("move-same-metadata");
    let original = root.join("data/original.jsonl");
    let old_bytes = b"{\"id\":\"same\",\"name\":\"before\"}\n";
    let new_bytes = b"{\"id\":\"same\",\"name\":\"after!\"}\n";
    assert_eq!(old_bytes.len(), new_bytes.len());
    std::fs::write(&original, old_bytes).unwrap();
    let index = crate::lifecycle::Index::start_build(
        &root,
        vec![intrinsic_definition("items")],
        RepositoryOptions::default(),
        crate::source::registry::SourceRegistry::builtin(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let before = index.view().unwrap();
    let handle = before.file_handle_for_path(&original).unwrap();
    let original_metadata = std::fs::metadata(&original).unwrap();
    std::fs::write(&original, new_bytes).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&original)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(original_metadata.modified().unwrap()))
        .unwrap();
    let destination = root.join("data/moved.jsonl");
    std::fs::rename(&original, &destination).unwrap();
    assert_eq!(
        SourceRevision::from_metadata(&std::fs::metadata(&destination).unwrap()).unwrap(),
        SourceRevision::from_metadata(&original_metadata).unwrap()
    );
    let moved = index.move_file(handle, &destination).unwrap();
    assert_eq!(moved.strategy, FileMutationStrategy::AppendReplacement);
    assert_eq!(moved.retired_records, 1);
    let hit = index
        .view()
        .unwrap()
        .get_record("items", "same", ReturnMode::Bytes)
        .unwrap()
        .unwrap();
    assert_eq!(
        hit.payload,
        Some(Payload::Bytes(new_bytes[..new_bytes.len() - 1].to_vec()))
    );
    let revision = index.status().unwrap().revision;
    let missing = index.move_file(moved.file, &root.join("data/missing.jsonl"));
    assert!(
        matches!(missing, Err(IndexError::IncompatibleConfiguration(_))),
        "{missing:?}"
    );
    assert_eq!(index.status().unwrap().revision, revision);
    assert_eq!(
        index.view().unwrap().file_handle_for_path(&destination),
        Some(moved.file)
    );
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn incompatible_moves_reindex_collection_reader_and_filename_identity() {
    let root = test_root("move-membership");
    std::fs::create_dir_all(root.join("data/left")).unwrap();
    std::fs::create_dir_all(root.join("data/right")).unwrap();
    let original = root.join("data/left/item.jsonl");
    std::fs::write(&original, b"{\"id\":\"same\"}\n").unwrap();
    let mut left = intrinsic_definition("left");
    left.source_path = "data/left".into();
    let mut right = intrinsic_definition("right");
    right.source_path = "data/right".into();
    let index = crate::lifecycle::Index::start_build(
        &root,
        vec![left, right],
        RepositoryOptions::default(),
        crate::source::registry::SourceRegistry::builtin(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let handle = index
        .view()
        .unwrap()
        .file_handle_for_path(&original)
        .unwrap();
    let destination = root.join("data/right/item.jsonl");
    std::fs::rename(&original, &destination).unwrap();
    let moved = index.move_file(handle, &destination).unwrap();
    assert_eq!(moved.strategy, FileMutationStrategy::AppendReplacement);
    let view = index.view().unwrap();
    assert!(
        view.get_record("left", "same", ReturnMode::Locator)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        view.get_record("right", "same", ReturnMode::Locator)
            .unwrap()
            .unwrap()
            .file
            .unwrap()
            .path,
        std::path::Path::new("data/right/item.jsonl")
    );
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();

    let root = test_root("move-reader");
    let original = root.join("data/item.jsonl");
    std::fs::write(&original, b"{\"id\":\"same\"}\n").unwrap();
    let mut json = intrinsic_definition("json");
    json.format = SourceFormat::Json;
    let index = crate::lifecycle::Index::start_build(
        &root,
        vec![intrinsic_definition("jsonl"), json],
        RepositoryOptions::default(),
        crate::source::registry::SourceRegistry::builtin(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let handle = index
        .view()
        .unwrap()
        .file_handle_for_path(&original)
        .unwrap();
    let destination = root.join("data/item.json");
    std::fs::rename(&original, &destination).unwrap();
    let moved = index.move_file(handle, &destination).unwrap();
    assert_eq!(moved.strategy, FileMutationStrategy::AppendReplacement);
    let view = index.view().unwrap();
    assert!(
        view.get_record("jsonl", "same", ReturnMode::Locator)
            .unwrap()
            .is_none()
    );
    assert!(
        view.get_record("json", "same", ReturnMode::Bytes)
            .unwrap()
            .is_some()
    );
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();

    let root = test_root("move-filename-id");
    let original = root.join("data/old.json");
    std::fs::write(&original, b"{\"name\":\"same\"}\n").unwrap();
    let mut definition = CollectionDefinition::folder("items", "data", SourceFormat::Json);
    definition.identity = IdentityPolicy::Filename;
    let index = crate::lifecycle::Index::start_build(
        &root,
        vec![definition],
        RepositoryOptions::default(),
        crate::source::registry::SourceRegistry::builtin(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let handle = index
        .view()
        .unwrap()
        .file_handle_for_path(&original)
        .unwrap();
    let destination = root.join("data/new.json");
    std::fs::rename(&original, &destination).unwrap();
    let moved = index.move_file(handle, &destination).unwrap();
    assert_eq!(moved.strategy, FileMutationStrategy::AppendReplacement);
    let view = index.view().unwrap();
    assert!(
        view.get_record("items", "old", ReturnMode::Locator)
            .unwrap()
            .is_none()
    );
    assert!(
        view.get_record("items", "new", ReturnMode::Locator)
            .unwrap()
            .is_some()
    );
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn compatible_move_routes_pinned_reads_filter_sort_cache_and_compaction() {
    use crate::query_results::Payload;

    let root = test_root("move-persistence-routing");
    let original = root.join("data/original.jsonl");
    let other = root.join("data/a-other.jsonl");
    std::fs::write(
        &original,
        b"{\"id\":\"A\",\"name\":\"alpha\"}\n{\"id\":\"B\",\"name\":\"beta\"}\n",
    )
    .unwrap();
    std::fs::write(&other, b"{\"id\":\"C\",\"name\":\"gamma\"}\n").unwrap();
    let definition = intrinsic_definition("items");
    let index = crate::lifecycle::Index::start_build(
        &root,
        vec![definition.clone()],
        RepositoryOptions::default(),
        crate::source::registry::SourceRegistry::builtin(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let pinned = index.view().unwrap();
    let handle = pinned.file_handle_for_path(&original).unwrap();
    let original_metadata = std::fs::metadata(&original).unwrap();
    let destination = root.join("data/z-moved.jsonl");
    std::fs::rename(&original, &destination).unwrap();
    let moved = index.move_file(handle, &destination).unwrap();
    assert_eq!(moved.strategy, FileMutationStrategy::ContentPreservingMove);
    assert_eq!(moved.retired_records, 0);

    let query = |filter: Option<&str>, mode| Query {
        collections: vec!["items".into()],
        fts: None,
        filter: filter.map(|text| parse_filter(text).unwrap()),
        options: QueryOptions {
            sort: vec![SortField::simple("@file.path", false)],
            limit: 10,
            offset: 0,
            count: true,
            return_mode: mode,
            evidence: false,
        },
    };
    let old_hit = pinned
        .get_record("items", "A", ReturnMode::Locator)
        .unwrap()
        .unwrap();
    assert_eq!(
        old_hit.file.as_ref().unwrap().path,
        std::path::Path::new("data/original.jsonl")
    );
    assert!(pinned.get_record("items", "A", ReturnMode::Bytes).is_err());

    let check_current = |index: &Index| {
        let view = index.view().unwrap();
        let all = view.search(&query(None, ReturnMode::Locator)).unwrap();
        assert_eq!(all.count, Some(3));
        assert_eq!(
            all.hits
                .iter()
                .map(|hit| hit.file.as_ref().expect("file origin").path.as_path())
                .collect::<Vec<_>>(),
            [
                std::path::Path::new("data/a-other.jsonl"),
                std::path::Path::new("data/z-moved.jsonl"),
                std::path::Path::new("data/z-moved.jsonl"),
            ]
        );
        let filtered = view
            .search(&query(Some("@file.path ~ 'z-moved'"), ReturnMode::Bytes))
            .unwrap();
        assert_eq!(filtered.count, Some(2));
        let ranged = view
            .search(&query(
                Some("@file.path >= 'data/z-moved' && @file.path < 'data/z-moved0'"),
                ReturnMode::Locator,
            ))
            .unwrap();
        assert_eq!(ranged.count, Some(2));
        assert!(
            ranged
                .hits
                .iter()
                .all(|hit| hit.file.as_ref().expect("file origin").path
                    == std::path::Path::new("data/z-moved.jsonl"))
        );
        let empty_range = view
            .search(&query(
                Some("@file.path >= 'data/z-moved' && @file.path < 'data/z-moved'"),
                ReturnMode::Locator,
            ))
            .unwrap();
        assert_eq!(empty_range.count, Some(0));
        assert!(
            filtered
                .hits
                .iter()
                .all(|hit| hit.file.as_ref().expect("file origin").path
                    == std::path::Path::new("data/z-moved.jsonl"))
        );
        let direct = view
            .get_record("items", "A", ReturnMode::Bytes)
            .unwrap()
            .unwrap();
        assert_eq!(
            direct.file.as_ref().unwrap().path,
            std::path::Path::new("data/z-moved.jsonl")
        );
        assert_eq!(
            direct.payload,
            Some(Payload::Bytes(
                b"{\"id\":\"A\",\"name\":\"alpha\"}".to_vec()
            ))
        );
    };
    check_current(&index);
    let unrelated = b"{\"id\":\"X\",\"name\":\"xxxxx\"}\n{\"id\":\"Y\",\"name\":\"xxxx\"}\n";
    assert_eq!(unrelated.len(), original_metadata.len() as usize);
    std::fs::write(&original, unrelated).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&original)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(original_metadata.modified().unwrap()))
        .unwrap();
    let replaced_revision =
        SourceRevision::from_metadata(&std::fs::metadata(&original).unwrap()).unwrap();
    let original_revision = SourceRevision::from_metadata(&original_metadata).unwrap();
    assert_eq!(replaced_revision.size, original_revision.size);
    assert_eq!(replaced_revision.modified_ns, original_revision.modified_ns);
    assert_ne!(replaced_revision.object_id, original_revision.object_id);
    assert_ne!(
        std::fs::metadata(&original).unwrap().created().ok(),
        original_metadata.created().ok()
    );
    assert!(pinned.get_record("items", "A", ReturnMode::Bytes).is_err());
    assert!(pinned.search(&query(None, ReturnMode::Bytes)).is_err());
    std::fs::remove_file(&original).unwrap();

    check_current(&index);
    index.compact().unwrap();
    check_current(&index);
    index.wait_for_reconciliation().unwrap();
    check_current(&index);
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "large move performance comparison; run explicitly in release mode"]
fn large_compatible_move_skips_extraction_and_posting_rewrites() {
    use std::io::Write;
    use std::sync::Arc;
    use std::time::Instant;

    const RECORDS: usize = 50_000;
    for compatible in [true, false] {
        let root = test_root(if compatible {
            "move-fast"
        } else {
            "move-fallback"
        });
        let original = root.join("data/original.jsonl");
        let mut writer = std::io::BufWriter::new(std::fs::File::create(&original).unwrap());
        for number in 0..RECORDS {
            writeln!(
                writer,
                "{{\"id\":\"item-{number}\",\"name\":\"item-{number}\"}}"
            )
            .unwrap();
        }
        writer.flush().unwrap();
        drop(writer);
        let index = crate::lifecycle::Index::start_build(
            &root,
            vec![intrinsic_definition("items")],
            RepositoryOptions::default(),
            crate::source::registry::SourceRegistry::builtin(),
        )
        .and_then(|task| task.wait_complete())
        .unwrap();
        let before = index.view().unwrap();
        let handle = before.file_handle_for_path(&original).unwrap();
        let old_shards = Arc::clone(&before.0.segments[0]);
        let destination = root.join("data/moved.jsonl");
        if compatible {
            std::fs::rename(&original, &destination).unwrap();
        } else {
            std::fs::copy(&original, &destination).unwrap();
            let mut destination_file = std::fs::OpenOptions::new()
                .append(true)
                .open(&destination)
                .unwrap();
            destination_file.write_all(b" \n").unwrap();
            std::fs::remove_file(&original).unwrap();
        }
        let digest_bytes = std::fs::metadata(&destination).unwrap().len();
        let started = Instant::now();
        let moved = index.move_file(handle, &destination).unwrap();
        let elapsed = started.elapsed();
        let after = index.view().unwrap();
        let new_shards = after.0.segments[0]
            .iter()
            .filter(|candidate| !old_shards.iter().any(|old| Arc::ptr_eq(candidate, old)))
            .collect::<Vec<_>>();
        let newly_parsed_records = new_shards
            .iter()
            .map(|shard| shard.index.record_parses)
            .sum::<u64>();
        let rewritten_posting_keys = new_shards
            .iter()
            .flat_map(|shard| shard.index.postings.fields.values())
            .map(|posting| posting.values.len())
            .sum::<usize>();
        let extraction_calls =
            usize::from(moved.strategy == FileMutationStrategy::AppendReplacement);
        println!(
            "move_case\t{}\t{:.3}\t{}\t{}\t{}\t{}\t{}",
            if compatible { "compatible" } else { "fallback" },
            elapsed.as_secs_f64() * 1000.0,
            digest_bytes,
            extraction_calls,
            newly_parsed_records,
            new_shards.len(),
            rewritten_posting_keys,
        );
        assert_eq!(moved.indexed_records, RECORDS as u64);
        assert_eq!(index.status().unwrap().live_records, RECORDS as u64);
        if compatible {
            assert_eq!(moved.strategy, FileMutationStrategy::ContentPreservingMove);
            assert_eq!(extraction_calls, 0);
            assert_eq!(newly_parsed_records, 0);
            assert_eq!(new_shards.len(), 0);
            assert_eq!(rewritten_posting_keys, 0);
        } else {
            assert_eq!(moved.strategy, FileMutationStrategy::AppendReplacement);
            assert_eq!(extraction_calls, 1);
            assert_eq!(newly_parsed_records, RECORDS as u64);
            assert!(new_shards.len() > 0);
            assert!(rewritten_posting_keys >= RECORDS);
        }
        index.shutdown().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn shared_partition_verified_update_preserves_its_record_slot() {
    let root = test_root("partition-slot-replace");
    let mut definition = CollectionDefinition::folder("items", "data", SourceFormat::JsonLines);
    definition.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
    definition
        .schema
        .register_field(FieldDefinition {
            path: vec!["id".into()],
            kind: LogicalType::Id,
            required: true,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
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
    definition.sort_fields = vec![FieldPath::parse("name").unwrap()];
    let first = root.join("data/first.jsonl");
    let middle = root.join("data/middle.jsonl");
    let last = root.join("data/last.jsonl");
    let old = b"{\"id\":\"middle\",\"name\":\"before\"}\n";
    std::fs::write(&first, b"{\"id\":\"first\",\"name\":\"first\"}\n").unwrap();
    std::fs::write(&middle, old).unwrap();
    std::fs::write(&last, b"{\"id\":\"last\",\"name\":\"last\"}\n").unwrap();
    let built = crate::lifecycle::Index::start_build(
        &root,
        vec![definition.clone()],
        RepositoryOptions::default(),
        crate::source::registry::SourceRegistry::builtin(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let restored = built;
    let pinned = restored.view().unwrap();
    let handle = pinned.file_handle_for_path(&middle).unwrap();
    let old_shard = pinned
        .segments(CollectionId(0))
        .unwrap()
        .iter()
        .find(|shard| shard.index.file(handle.file_id).is_some())
        .unwrap();
    assert_eq!(old_shard.index.records.len(), 3);
    let old_slot = old_shard.index.file_records(handle.file_id).unwrap()[0];
    let old_ordinal = old_shard.id.get();
    std::fs::write(&middle, b"{\"id\":\"middle\",\"name\":\"after!\"}\n").unwrap();
    let result = restored
        .update_file_with_options(
            handle,
            &middle,
            UpdateOptions {
                old_content: Some(old),
                strategy: UpdateStrategy::PreferVerified,
            },
        )
        .unwrap();
    assert_eq!(
        result.strategy,
        FileMutationStrategy::VerifiedSlotPreserving
    );
    let current = restored.view().unwrap();
    let new_shard = current
        .segments(CollectionId(0))
        .unwrap()
        .iter()
        .find(|shard| shard.index.file(handle.file_id).is_some())
        .unwrap();
    assert_eq!(new_shard.id.get(), old_ordinal);
    assert_eq!(
        new_shard.index.file_records(handle.file_id).unwrap(),
        [old_slot]
    );
    assert_eq!(new_shard.index.records.len(), 3);
    assert_eq!(new_shard.active.len(), 3);
    assert_eq!(
        old_shard.index.file_records(handle.file_id).unwrap(),
        [old_slot]
    );
    let query = Query {
        collections: vec!["items".into()],
        fts: Some(FtsQuery {
            case_sensitivity: CaseSensitivity::Sensitive,
            mode: FtsMode::Literal,
            literal: "after!".into(),
            pattern_parameter: None,
            fields: vec![],
        }),
        filter: None,
        options: QueryOptions {
            sort: vec![SortField::simple("name", false)],
            limit: 10,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    assert_eq!(current.search(&query).unwrap().count, Some(1));
    restored.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn supplied_bytes_update_preserves_source_and_checks_deletion_conflict() {
    let root = test_root("bytes");
    let index = index(&root);
    let path = root.join("data/file.jsonl");
    std::fs::write(&path, b"{\"name\":\"old\"}\n").unwrap();
    let added = index.add_file(&path).unwrap();

    let bytes = b"{\"name\":\"first\"}\n{\"name\":\"second\"}\n";
    std::fs::write(&path, bytes).unwrap();
    let revision = SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap();
    let updated = index
        .update_file_from_bytes(added.file, &path, bytes, revision)
        .unwrap();
    assert_eq!(updated.kind, FileMutationKind::Replaced);
    assert_eq!(updated.retired_records, 1);
    assert_eq!(updated.indexed_records, 2);
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    let same = index
        .update_file_from_bytes(updated.file, &path, bytes, revision)
        .unwrap();
    assert_eq!(same.kind, FileMutationKind::Unchanged);
    assert_eq!(same.revision, updated.revision);
    assert_eq!(same.file, updated.file);
    assert_eq!(std::fs::read(&path).unwrap(), bytes);

    let mut different = bytes.to_vec();
    different[0] = b'[';
    assert!(matches!(
        index.update_file_from_bytes(updated.file, &path, &different, revision),
        Err(IndexError::SourceUnavailable(_))
    ));
    assert_eq!(index.status().unwrap().revision, updated.revision);

    let wrong = SourceRevision {
        size: revision.size + 1,
        ..revision
    };
    assert!(matches!(
        index.update_file_from_bytes(updated.file, &path, bytes, wrong),
        Err(IndexError::SourceUnavailable(_))
    ));
    assert_eq!(index.status().unwrap().revision, updated.revision);

    std::fs::remove_file(&path).unwrap();
    index.delete_file(updated.file).unwrap();
    assert!(matches!(
        index.update_file_from_bytes(updated.file, &path, bytes, revision),
        Err(IndexError::FileConflict { .. })
    ));
    assert!(!path.exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn prepared_add_rejects_same_stamp_path_replacement_before_publication() {
    let root = test_root("same-stamp-object-swap");
    let index = index(&root);
    let path = root.join("data/file.jsonl");
    let replacement = root.join("data/replacement.jsonl");
    let original = b"{\"name\":\"aaa\"}\n";
    let changed = b"{\"name\":\"bbb\"}\n";
    std::fs::write(&path, original).unwrap();
    let metadata = std::fs::metadata(&path).unwrap();
    let prepared = index.prepare_add_file(&path).unwrap();
    let source = prepared.file.source.clone();
    let identity = prepared.file.identity;
    let fingerprint = prepared.file.fingerprint;
    let old_revision = index.view().unwrap().revision();
    let staged = index.stage_prepared_add(prepared).unwrap();

    std::fs::write(&replacement, changed).unwrap();
    std::fs::File::open(&replacement)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(metadata.modified().unwrap()))
        .unwrap();
    std::fs::rename(&replacement, &path).unwrap();
    let replaced_revision =
        SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap();
    assert_eq!(replaced_revision.size, source.revision.size);
    assert_eq!(replaced_revision.modified_ns, source.revision.modified_ns);
    assert_ne!(replaced_revision.object_id, source.revision.object_id);
    assert!(
        crate::construction::file_preparation::verify_current_source(
            &root,
            &source,
            identity,
            fingerprint,
        )
        .is_err()
    );
    drop(staged);
    assert_eq!(index.view().unwrap().revision(), old_revision);
    let added = index.add_file(&path).unwrap();
    assert_eq!(added.indexed_records, 1);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn move_rejects_destination_owned_by_another_file() {
    let root = test_root("collision");
    let index = index(&root);
    let first = root.join("data/one.jsonl");
    let second = root.join("data/two.jsonl");
    std::fs::write(&first, b"{\"name\":\"one\"}\n").unwrap();
    std::fs::write(&second, b"{\"name\":\"two\"}\n").unwrap();
    let one = index.add_file(&first).unwrap();
    index.add_file(&second).unwrap();
    let before = index.status().unwrap();
    assert!(matches!(
        index.move_file(one.file, &second),
        Err(IndexError::DestinationExists)
    ));
    assert_eq!(index.status().unwrap(), before);
    assert!(first.exists() && second.exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn verified_update_preserves_isolated_slot_and_bad_hint_falls_back() {
    let root = test_root("verified");
    let index = crate::lifecycle::Index::start_build(
        &root,
        vec![intrinsic_definition("items")],
        RepositoryOptions::default(),
        crate::source::registry::SourceRegistry::builtin(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let path = root.join("data/verified.jsonl");
    let old = b"{\"id\":\"same\",\"name\":\"old\"}\n";
    std::fs::write(&path, old).unwrap();
    let added = index.add_file(&path).unwrap();
    let pinned = index.view().unwrap();
    let old_ordinal = pinned.segments(CollectionId(0)).unwrap()[0].id.get();

    let new = b"{\"id\":\"same\",\"name\":\"new\"}\n";
    std::fs::write(&path, new).unwrap();
    let verified = index
        .update_file_with_options(
            added.file,
            &path,
            UpdateOptions {
                old_content: Some(old),
                strategy: UpdateStrategy::PreferVerified,
            },
        )
        .unwrap();
    assert_eq!(
        verified.strategy,
        FileMutationStrategy::VerifiedSlotPreserving
    );
    assert_eq!(verified.fallback_reason, None);
    assert_eq!(verified.previous_records, 1);
    assert_eq!(verified.indexed_records, 1);
    assert_eq!(
        index
            .view()
            .unwrap()
            .segments(CollectionId(0))
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        index.view().unwrap().segments(CollectionId(0)).unwrap()[0]
            .id
            .get(),
        old_ordinal
    );
    assert_eq!(
        pinned.segments(CollectionId(0)).unwrap()[0].id.get(),
        old_ordinal
    );
    assert_eq!(pinned.segments(CollectionId(0)).unwrap()[0].active.len(), 1);

    let latest = b"{\"id\":\"same\",\"name\":\"latest\"}\n";
    std::fs::write(&path, latest).unwrap();
    let fallback = index
        .update_file_with_options(
            verified.file,
            &path,
            UpdateOptions {
                old_content: Some(b"wrong old bytes"),
                strategy: UpdateStrategy::PreferVerified,
            },
        )
        .unwrap();
    assert_eq!(fallback.strategy, FileMutationStrategy::AppendReplacement);
    assert_eq!(
        fallback.fallback_reason,
        Some(VerifiedFallbackReason::ContentMismatch)
    );
    assert_eq!(index.status().unwrap().live_records, 1);
    assert_eq!(
        index
            .view()
            .unwrap()
            .segments(CollectionId(0))
            .unwrap()
            .len(),
        2
    );

    std::fs::remove_file(&path).unwrap();
    let deleted = index
        .delete_file_with_old_content(fallback.file, latest)
        .unwrap();
    assert_eq!(
        deleted.deletion.strategy,
        DeleteStrategy::VerifiedPhysicalRemoval
    );
    assert_eq!(deleted.fallback_reason, None);
    assert_eq!(index.status().unwrap().live_records, 0);
    assert!(
        index
            .view()
            .unwrap()
            .segments(CollectionId(0))
            .unwrap()
            .is_empty()
    );
    assert_eq!(pinned.segments(CollectionId(0)).unwrap()[0].active.len(), 1);
    assert!(!path.exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn fresh_base_verified_update_compacts_staged_root_and_keeps_pinned_view() {
    use sha2::{Digest, Sha256};

    let root = test_root("base-verified-update");
    let path = root.join("data/base.jsonl");
    let old = b"{\"name\":\"old\"}\n";
    std::fs::write(&path, old).unwrap();
    let index = index(&root);
    let pinned = index.view().unwrap();
    let handle = pinned.file_handle_for_path(&path).unwrap();
    assert_eq!(
        pinned.file_fingerprint(handle.file_id),
        Some(Sha256::digest(old).into())
    );
    assert_eq!(
        pinned.0.active[0].len()
            + pinned
                .segments(CollectionId(0))
                .unwrap()
                .iter()
                .map(|shard| shard.active.len())
                .sum::<u64>(),
        1
    );
    let unchanged = index.update_file(handle, &path).unwrap();
    assert_eq!(unchanged.kind, FileMutationKind::Unchanged);
    assert_eq!(unchanged.revision, pinned.revision());
    assert_eq!(unchanged.file, handle);
    std::fs::write(&path, b"{\"name\":\"new\"}\n").unwrap();
    let updated = index
        .update_file_with_options(
            handle,
            &path,
            UpdateOptions {
                old_content: Some(old),
                strategy: UpdateStrategy::PreferVerified,
            },
        )
        .unwrap();
    assert_eq!(
        updated.strategy,
        FileMutationStrategy::VerifiedPhysicalRemoval
    );
    assert_eq!(updated.fallback_reason, None);
    let current = index.view().unwrap();
    assert_eq!(current.repository().collections()[0].id_lookup.len(), 1);
    assert_eq!(current.segments(CollectionId(0)).unwrap().len(), 1);
    assert_eq!(
        pinned.0.active[0].len()
            + pinned
                .segments(CollectionId(0))
                .unwrap()
                .iter()
                .map(|shard| shard.active.len())
                .sum::<u64>(),
        1
    );
    assert_eq!(index.status().unwrap().live_records, 1);
    assert_eq!(
        index.view().unwrap().file_fingerprint(handle.file_id),
        Some(Sha256::digest(b"{\"name\":\"new\"}\n").into())
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn fresh_base_verified_delete_reclaims_base_without_reading_source() {
    let root = test_root("base-verified-delete");
    let path = root.join("data/base.jsonl");
    let old = b"{\"name\":\"old\"}\n";
    std::fs::write(&path, old).unwrap();
    let index = index(&root);
    let pinned = index.view().unwrap();
    let handle = pinned.file_handle_for_path(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    let deleted = index.delete_file_with_old_content(handle, old).unwrap();
    assert_eq!(
        deleted.deletion.strategy,
        DeleteStrategy::VerifiedPhysicalRemoval
    );
    assert_eq!(deleted.fallback_reason, None);
    let current = index.view().unwrap();
    assert!(current.segments(CollectionId(0)).unwrap().is_empty());
    assert_eq!(index.status().unwrap().live_records, 0);
    assert_eq!(
        pinned.0.active[0].len()
            + pinned
                .segments(CollectionId(0))
                .unwrap()
                .iter()
                .map(|shard| shard.active.len())
                .sum::<u64>(),
        1
    );
    assert!(!path.exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn verified_removal_compacts_a_shared_shard_without_losing_siblings() {
    let root = test_root("shared-shard");
    let index = index(&root);
    for number in 0..65 {
        let path = root.join(format!("data/{number}.jsonl"));
        std::fs::write(&path, format!("{{\"name\":\"item-{number}\"}}\n")).unwrap();
        index.add_file(&path).unwrap();
    }
    let before = index.view().unwrap();
    let shared = before
        .segments(CollectionId(0))
        .unwrap()
        .iter()
        .find(|shard| shard.index.files.len() > 1)
        .expect("segment consolidation should share a shard");
    let target = shared.index.files[0].entry.id;
    let sibling = shared.index.files[1].entry.id;
    let handle = before.file_handle(target).unwrap();
    let path = root.join(&before.file_entry(target).unwrap().relative_path);
    let old = std::fs::read(&path).unwrap();
    std::fs::write(&path, b"{\"name\":\"changed\"}\n").unwrap();
    let result = index
        .update_file_with_options(
            handle,
            &path,
            UpdateOptions {
                old_content: Some(&old),
                strategy: UpdateStrategy::PreferVerified,
            },
        )
        .unwrap();
    assert_eq!(result.fallback_reason, None);
    assert_eq!(
        result.strategy,
        FileMutationStrategy::VerifiedPhysicalRemoval
    );
    let current = index.view().unwrap();
    assert_eq!(current.file_record_count(sibling), Some(1));
    assert_eq!(current.file_record_count(target), Some(1));
    assert_eq!(index.status().unwrap().live_records, 65);
    let query = Query {
        collections: vec!["items".into()],
        fts: None,
        filter: None,
        options: QueryOptions {
            sort: Vec::new(),
            limit: 0,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    assert_eq!(current.count_exact(&query).unwrap(), 65);
    assert_eq!(before.file_record_count(target), Some(1));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn verified_many_to_zero_keeps_file_active_and_bad_delete_hint_tombstones() {
    let root = test_root("zero");
    let index = index(&root);
    let path = root.join("data/zero.jsonl");
    let old = b"{\"n\":1}\n{\"n\":2}\n";
    std::fs::write(&path, old).unwrap();
    let added = index.add_file(&path).unwrap();
    std::fs::write(&path, b"").unwrap();
    let updated = index
        .update_file_with_options(
            added.file,
            &path,
            UpdateOptions {
                old_content: Some(old),
                strategy: UpdateStrategy::PreferVerified,
            },
        )
        .unwrap();
    assert_eq!(
        updated.strategy,
        FileMutationStrategy::VerifiedPhysicalRemoval
    );
    assert_eq!(updated.previous_records, 2);
    assert_eq!(updated.indexed_records, 0);
    assert_eq!(index.status().unwrap().indexed_files, 1);
    assert_eq!(index.status().unwrap().live_records, 0);
    assert_eq!(
        index
            .view()
            .unwrap()
            .file_record_count(updated.file.file_id),
        Some(0)
    );

    std::fs::remove_file(&path).unwrap();
    let deletion = index
        .delete_file_with_old_content(updated.file, b"wrong old bytes")
        .unwrap();
    assert_eq!(deletion.deletion.strategy, DeleteStrategy::Tombstone);
    assert_eq!(
        deletion.fallback_reason,
        Some(VerifiedFallbackReason::ContentMismatch)
    );
    assert_eq!(index.status().unwrap().indexed_files, 0);
    assert!(!path.exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn verified_update_removes_departed_collection_membership() {
    let root = test_root("membership");
    std::fs::create_dir_all(root.join("other")).unwrap();
    let index = crate::lifecycle::Index::start_build(
        &root,
        vec![
            CollectionDefinition::folder("left", "data", SourceFormat::JsonLines),
            CollectionDefinition::folder("right", "other", SourceFormat::JsonLines),
        ],
        RepositoryOptions::default(),
        crate::source::registry::SourceRegistry::builtin(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let source = root.join("data/entry.jsonl");
    let destination = root.join("other/entry.jsonl");
    let bytes = b"{\"name\":\"entry\"}\n";
    std::fs::write(&source, bytes).unwrap();
    let added = index.add_file(&source).unwrap();
    std::fs::rename(&source, &destination).unwrap();
    let changed = index
        .update_file_with_options(
            added.file,
            &destination,
            UpdateOptions {
                old_content: Some(bytes),
                strategy: UpdateStrategy::PreferVerified,
            },
        )
        .unwrap();
    assert_eq!(
        changed.strategy,
        FileMutationStrategy::VerifiedPhysicalRemoval
    );
    assert_eq!(changed.affected_collections, ["left", "right"]);
    let view = index.view().unwrap();
    assert!(view.segments(CollectionId(0)).unwrap().is_empty());
    assert_eq!(view.file_record_count(changed.file.file_id), Some(1));
    assert_eq!(view.segments(CollectionId(1)).unwrap().len(), 1);
    assert!(!source.exists() && destination.exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn slot_preserving_replacement_keeps_local_address_and_rebuilds_all_routes() {
    let root = test_root("slot-routes");
    std::fs::create_dir_all(root.join("sources")).unwrap();
    std::fs::create_dir_all(root.join("targets")).unwrap();
    std::fs::write(root.join("targets/t.json"), b"{\"id\":\"T\"}").unwrap();
    let mut target_schema = Schema::default();
    target_schema
        .register_field(FieldDefinition {
            path: vec!["id".into()],
            kind: LogicalType::Id,
            required: true,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    let mut targets = CollectionDefinition::folder("targets", "targets", SourceFormat::Json);
    targets.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
    targets.schema = target_schema;
    let mut source_schema = Schema::default();
    for (field, kind) in [
        ("id", LogicalType::Id),
        ("entry", LogicalType::Id),
        ("score", LogicalType::Number),
        ("title", LogicalType::String),
    ] {
        source_schema
            .register_field(FieldDefinition {
                path: vec![field.into()],
                kind,
                required: true,
                nullable: false,
                indexed: true,
                multi: false,
                element_nullable: vec![],
            })
            .unwrap();
    }
    source_schema
        .register_relation(RelationDefinition {
            path: vec!["entry".into()],
            target_collection: "targets".into(),
            cardinality: RelationCardinality::One,
            reverse_alias: Some("sources".into()),
        })
        .unwrap();
    let mut sources = CollectionDefinition::folder("sources", "sources", SourceFormat::JsonLines);
    sources.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
    sources.schema = source_schema;
    sources.sort_fields = vec![FieldPath::parse("score").unwrap()];
    let definitions = vec![targets, sources];
    let index = crate::lifecycle::Index::start_build(
        &root,
        definitions.clone(),
        RepositoryOptions::default(),
        crate::source::registry::SourceRegistry::builtin(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let path = root.join("sources/s.jsonl");
    let other_path = root.join("sources/r.jsonl");
    let original = b"{\"id\":\"S\",\"entry\":\"T\",\"score\":1,\"title\":\"alpha\"}\n";
    std::fs::write(&path, original).unwrap();
    let added = index.add_file(&path).unwrap();
    std::fs::write(
        &other_path,
        b"{\"id\":\"R\",\"entry\":\"T\",\"score\":2,\"title\":\"other\"}\n",
    )
    .unwrap();
    index.add_file(&other_path).unwrap();
    let relation_path = vec!["entry".into()];
    let pinned = index.view().unwrap();
    let original_locator = pinned
        .relation_outgoing_page("sources", &relation_path, "S", 0, 10)
        .unwrap()
        .source;
    assert_eq!(original_locator.slot, RecordSlot(0));
    let ordinal = original_locator.segment_ordinal.unwrap();
    let sorted = Query {
        collections: vec!["sources".into()],
        fts: None,
        filter: None,
        options: QueryOptions {
            sort: vec![SortField::simple("score", false)],
            limit: 10,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    let ids = |view: &crate::lifecycle::IndexView| {
        view.search(&sorted)
            .unwrap()
            .hits
            .into_iter()
            .map(|hit| hit.record_id)
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(&pinned), ["S", "R"]);

    let same_id = b"{\"id\":\"S\",\"entry\":\"T\",\"score\":3,\"title\":\"bravo replacement\"}\n";
    std::fs::write(&path, same_id).unwrap();
    let first = index
        .update_file_with_options(
            added.file,
            &path,
            UpdateOptions {
                old_content: Some(original),
                strategy: UpdateStrategy::PreferVerified,
            },
        )
        .unwrap();
    assert_eq!(first.strategy, FileMutationStrategy::VerifiedSlotPreserving);
    let after_first = index.view().unwrap();
    let first_locator = after_first
        .relation_outgoing_page("sources", &relation_path, "S", 0, 10)
        .unwrap()
        .source;
    assert_eq!(first_locator.segment_ordinal, Some(ordinal));
    assert_eq!(first_locator.slot, RecordSlot(0));
    assert_ne!(first_locator.generation, original_locator.generation);
    assert_eq!(
        pinned
            .relation_outgoing_page("sources", &relation_path, "S", 0, 10)
            .unwrap()
            .source,
        original_locator
    );
    assert_eq!(ids(&after_first), ["R", "S"]);
    let mut score_three = sorted.clone();
    score_three.filter = Some(parse_filter("score = 3").unwrap());
    assert_eq!(after_first.search(&score_three).unwrap().count, Some(1));
    assert_eq!(
        after_first.search(&score_three).unwrap().hits,
        after_first.search_reference(&score_three).unwrap().hits
    );
    let mut text = sorted.clone();
    text.fts = Some(FtsQuery {
        case_sensitivity: CaseSensitivity::Sensitive,
        mode: FtsMode::Literal,
        literal: "bravo".into(),
        pattern_parameter: None,
        fields: vec![],
    });
    assert_eq!(after_first.search(&text).unwrap().count, Some(1));
    assert_eq!(
        after_first.search(&text).unwrap().hits,
        after_first.search_reference(&text).unwrap().hits
    );
    let backlink = after_first
        .relation_backlink_page("targets", "sources", "T", 0, 10)
        .unwrap();
    assert_eq!(backlink.total_sources, 2);
    assert!(backlink.sources.iter().any(|source| source.id == "S"));

    let changed_id =
        b"{\"id\":\"S2\",\"entry\":\"T\",\"score\":0,\"title\":\"charlie replacement\"}\n";
    std::fs::write(&path, changed_id).unwrap();
    let second = index
        .update_file_with_options(
            first.file,
            &path,
            UpdateOptions {
                old_content: Some(same_id),
                strategy: UpdateStrategy::PreferVerified,
            },
        )
        .unwrap();
    assert_eq!(
        second.strategy,
        FileMutationStrategy::VerifiedPhysicalRemoval
    );
    let current = index.view().unwrap();
    let changed_locator = current
        .relation_outgoing_page("sources", &relation_path, "S2", 0, 10)
        .unwrap()
        .source;
    assert_ne!(changed_locator.segment_ordinal, Some(ordinal));
    assert_eq!(changed_locator.slot, RecordSlot(0));
    assert_ne!(changed_locator.generation, first_locator.generation);
    assert!(
        current
            .get_record("sources", "S", ReturnMode::Locator)
            .unwrap()
            .is_none()
    );
    assert_eq!(ids(&current), ["S2", "R"]);
    assert_eq!(
        current
            .relation_backlink_page("targets", "sources", "T", 0, 10)
            .unwrap()
            .total_sources,
        2
    );
    assert!(
        current
            .relation_backlink_page("targets", "sources", "T", 0, 10)
            .unwrap()
            .sources
            .iter()
            .any(|source| source.id == "S2")
    );

    let loaded = current;
    assert_eq!(ids(&loaded), ["S2", "R"]);
    let restored_locator = loaded
        .relation_outgoing_page("sources", &relation_path, "S2", 0, 10)
        .unwrap()
        .source;
    assert_eq!(
        restored_locator.segment_ordinal,
        changed_locator.segment_ordinal
    );
    assert_eq!(restored_locator.slot, RecordSlot(0));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn incompatible_one_record_class_uses_verified_physical_removal() {
    let root = test_root("slot-class-fallback");
    let index = index(&root);
    let path = root.join("data/item.jsonl");
    let old = b"{\"name\":\"valid\"}\n";
    std::fs::write(&path, old).unwrap();
    let added = index.add_file(&path).unwrap();
    let old_ordinal = index.view().unwrap().segments(CollectionId(0)).unwrap()[0]
        .id
        .get();
    std::fs::write(&path, b"{broken}\n").unwrap();
    let updated = index
        .update_file_with_options(
            added.file,
            &path,
            UpdateOptions {
                old_content: Some(old),
                strategy: UpdateStrategy::PreferVerified,
            },
        )
        .unwrap();
    assert_eq!(
        updated.strategy,
        FileMutationStrategy::VerifiedPhysicalRemoval
    );
    assert_eq!(updated.fallback_reason, None);
    let current = index.view().unwrap();
    assert_ne!(
        current.segments(CollectionId(0)).unwrap()[0].id.get(),
        old_ordinal
    );
    assert_eq!(index.status().unwrap().live_records, 1);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn one_record_shared_file_preserves_every_collection_slot_atomically() {
    let root = test_root("shared-slot");
    let index = crate::lifecycle::Index::start_build(
        &root,
        vec![intrinsic_definition("left"), intrinsic_definition("right")],
        RepositoryOptions::default(),
        crate::source::registry::SourceRegistry::builtin(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let path = root.join("data/shared.jsonl");
    let old = b"{\"id\":\"same\",\"name\":\"old\"}\n";
    std::fs::write(&path, old).unwrap();
    let added = index.add_file(&path).unwrap();
    let pinned = index.view().unwrap();
    let ordinals = [CollectionId(0), CollectionId(1)]
        .map(|collection| pinned.segments(collection).unwrap()[0].id.get());
    std::fs::write(&path, b"{\"id\":\"same\",\"name\":\"new value\"}\n").unwrap();
    let updated = index
        .update_file_with_options(
            added.file,
            &path,
            UpdateOptions {
                old_content: Some(old),
                strategy: UpdateStrategy::PreferVerified,
            },
        )
        .unwrap();
    assert_eq!(
        updated.strategy,
        FileMutationStrategy::VerifiedSlotPreserving
    );
    assert_eq!(updated.previous_records, 2);
    assert_eq!(updated.indexed_records, 2);
    let current = index.view().unwrap();
    for (collection, ordinal) in [CollectionId(0), CollectionId(1)].into_iter().zip(ordinals) {
        let shards = current.segments(collection).unwrap();
        assert_eq!(shards.len(), 1);
        assert_eq!(shards[0].id.get(), ordinal);
        assert_eq!(shards[0].index.records[0].slot, RecordSlot(0));
        assert_eq!(pinned.segments(collection).unwrap()[0].id.get(), ordinal);
    }
    assert_eq!(index.status().unwrap().live_records, 2);
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn programmatic_rebuild_rejects_source_replacement_at_handoff() {
    let root = test_root("rebuild-handoff");
    let mut builder = Index::builder();
    let collection = builder.collection("items", Schema::default()).unwrap();
    builder
        .register_file_catalog(crate::files::FileCatalog::new(&root).unwrap())
        .unwrap();
    builder
        .register_file_source(
            collection,
            CollectionDefinition::folder("items", "data", SourceFormat::JsonLines).file_source(),
        )
        .unwrap();
    let index = builder.build().unwrap();
    let path = root.join("data/file.jsonl");
    std::fs::write(&path, b"{\"name\":\"aaa\"}\n").unwrap();
    let imported = index.add_file(&path).unwrap();
    let pinned = index.view().unwrap();
    let expected = std::fs::metadata(&path).unwrap();
    let result = index.rebuild_with_before_publish(|_| {
        let substitute = root.join("replacement");
        std::fs::write(&substitute, b"{\"name\":\"bbb\"}\n").unwrap();
        std::fs::File::open(&substitute)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(expected.modified().unwrap()))
            .unwrap();
        std::fs::rename(&substitute, &path).unwrap();
    });
    assert!(matches!(result, Err(IndexError::SourceUnavailable(_))));
    assert_eq!(index.view().unwrap().revision(), pinned.revision());
    assert_eq!(
        index
            .view()
            .unwrap()
            .file_handle_for_path(std::path::Path::new("data/file.jsonl")),
        Some(imported.file)
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn multisegment_managed_file_has_one_membership_per_collection() {
    use crate::index::segment::SegmentPolicy;
    use std::num::NonZeroUsize;

    let root = test_root("distinct-collection-membership");
    let mut builder = Index::builder().with_options(RepositoryOptions {
        segment_policy: SegmentPolicy {
            target_records: NonZeroUsize::new(512).unwrap(),
            max_files: NonZeroUsize::new(1).unwrap(),
        },
        ..Default::default()
    });
    builder
        .register_file_catalog(crate::files::FileCatalog::new(&root).unwrap())
        .unwrap();
    for name in ["entries", "references"] {
        let definition = intrinsic_definition(name);
        let collection = builder.collection(name, definition.schema.clone()).unwrap();
        builder
            .register_file_source(collection, definition.file_source())
            .unwrap();
    }
    let index = builder.build().unwrap();
    let path = root.join("data/item.jsonl");
    let bytes = (0..768)
        .map(|n| format!("{{\"id\":\"r{n}\"}}\n"))
        .collect::<String>();
    std::fs::write(&path, bytes).unwrap();
    let added = index.add_file(&path).unwrap();
    assert_eq!(added.indexed_records, 1536);
    let pinned = index.view().unwrap();
    let catalog = pinned.file_catalog().unwrap();
    let id = added.file.file_id;
    assert_eq!(
        catalog.file(id).unwrap().collections,
        &[CollectionId(0), CollectionId(1)]
    );
    assert_eq!(catalog.records_for_file(id).unwrap().len(), 1536);
    for collection in ["entries", "references"] {
        assert_eq!(
            catalog
                .records_for_file_in_collection(id, collection)
                .unwrap()
                .len(),
            768
        );
    }
    for collection in &pinned.0.segments {
        assert_eq!(
            collection
                .iter()
                .map(|segment| segment.active.len())
                .collect::<Vec<_>>(),
            [512, 256]
        );
    }
    let moved_path = root.join("data/moved.jsonl");
    std::fs::rename(&path, &moved_path).unwrap();
    let moved = index.move_file(added.file, &moved_path).unwrap();
    assert_eq!(moved.indexed_records, 1536);
    assert_eq!(
        index.update_file(moved.file, &moved_path).unwrap().kind,
        FileMutationKind::Unchanged
    );
    std::fs::write(&moved_path, b"{\"id\":\"r0\"}\n{\"id\":\"r1\"}\n").unwrap();
    let replacement = index.update_file(moved.file, &moved_path).unwrap();
    assert_eq!(
        (replacement.previous_records, replacement.indexed_records),
        (1536, 4)
    );
    assert_eq!(
        index
            .view()
            .unwrap()
            .file_catalog()
            .unwrap()
            .records_for_file(id)
            .unwrap()
            .len(),
        4
    );
    assert_eq!(catalog.records_for_file(id).unwrap().len(), 1536);
    std::fs::remove_dir_all(root).unwrap();
}

type FilePreparationProbe = Box<dyn FnMut(&str) -> bool>;

thread_local! {
    static FILE_PREPARATION_PROBE: std::cell::RefCell<Option<FilePreparationProbe>> =
        const { std::cell::RefCell::new(None) };
}

pub(super) fn reach_file_preparation_checkpoint(phase: &str) -> bool {
    FILE_PREPARATION_PROBE.with_borrow_mut(|probe| probe.as_mut().is_some_and(|probe| probe(phase)))
}

#[test]
fn direct_publication_progresses_during_private_file_work() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    };
    use std::time::Duration;

    for phase in [
        "old-layout",
        "verified-compaction",
        "whole-document-finalization",
        "digest-read",
    ] {
        let root = test_root(phase);
        let mut builder = Index::builder();
        let mut definition = CollectionDefinition::folder("entries", "data", SourceFormat::Json);
        definition.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
        let collection = builder
            .collection("entries", definition.schema.clone())
            .unwrap();
        let registration = builder.register_records(collection, "direct", 1).unwrap();
        builder
            .register_file_catalog(crate::files::FileCatalog::new(&root).unwrap())
            .unwrap();
        builder
            .register_file_source(collection, definition.file_source())
            .unwrap();
        let index = builder.build().unwrap();
        let path = root.join("data/item.json");
        let old = br#"{"id":"old"}"#;
        let updating = phase == "old-layout" || phase == "verified-compaction";
        let previous = if updating {
            std::fs::write(&path, old).unwrap();
            Some(index.add_file(&path).unwrap().file)
        } else {
            None
        };
        let pinned = index.view().unwrap();
        std::fs::write(&path, br#"{"id":"new-record"}"#).unwrap();
        let session = index.ingestion();
        let writer = session.writer(registration).unwrap();
        let (entered, waiting) = mpsc::sync_channel(1);
        let (release, released) = mpsc::channel::<()>();
        let (published, receipt) = mpsc::sync_channel(1);
        let deadline = Duration::from_secs(5);
        let layouts = Arc::new(AtomicUsize::new(0));
        std::thread::scope(|scope| {
            let index_ref = &index;
            let path_ref = &path;
            let layouts_ref = Arc::clone(&layouts);
            let mutation = scope.spawn(move || {
                let mut paused = false;
                FILE_PREPARATION_PROBE.with_borrow_mut(|probe| {
                    *probe = Some(Box::new(move |step| {
                        if step == "old-layout" {
                            layouts_ref.fetch_add(1, Ordering::Relaxed);
                        }
                        if step == phase && !paused {
                            paused = true;
                            entered.send(()).unwrap();
                            let _ = released.recv_timeout(deadline * 2);
                        }
                        phase == "digest-read" && step == "force-digest"
                    }));
                });
                match previous {
                    Some(file) => index_ref.update_file_with_options(
                        file,
                        path_ref,
                        UpdateOptions {
                            strategy: UpdateStrategy::PreferVerified,
                            old_content: Some(old),
                        },
                    ),
                    None => index_ref.add_file(path_ref),
                }
            });
            let reached = waiting.recv_timeout(deadline);
            let direct = scope.spawn(move || {
                let mut batch = writer.batch();
                batch
                    .insert("direct", crate::model::Value::String("direct".into()))
                    .unwrap();
                let result = writer.submit(batch).unwrap().wait_published();
                let _ = published.send(result);
            });
            let outcome = receipt.recv_timeout(deadline);
            drop(release);
            reached.expect("file checkpoint reached");
            outcome
                .expect("direct receipt published while file was gated")
                .unwrap();
            direct.join().unwrap();
            let result = mutation.join().unwrap().unwrap();
            if updating {
                assert!(matches!(
                    result.strategy,
                    FileMutationStrategy::VerifiedPhysicalRemoval
                        | FileMutationStrategy::VerifiedSlotPreserving
                ));
            }
        });
        assert_eq!(layouts.load(Ordering::Relaxed), usize::from(updating));
        let live = index.view().unwrap();
        let contains = |view: &crate::lifecycle::IndexView, id| {
            view.get_record("entries", id, ReturnMode::Locator)
                .unwrap()
                .is_some()
        };
        assert!(contains(&live, "new-record"));
        assert!(contains(&live, "direct"));
        assert!(!contains(&live, "old"));
        assert!(!contains(&pinned, "direct"));
        assert_eq!(contains(&pinned, "old"), updating);
        session.finish().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn managed_streams_bound_framers_pending_input_and_oversize_decoder_work() {
    use crate::inventory::ByteRange;
    use crate::source::{BuildChunk, ExtractedRecord, SourceReader};
    use std::io::Read;
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    };
    use std::time::{Duration, Instant};

    struct BudgetedLines {
        core_jsonl: bool,
        gate: AtomicBool,
        entered: mpsc::SyncSender<()>,
        released: Mutex<mpsc::Receiver<()>>,
    }

    impl SourceReader for BudgetedLines {
        fn name(&self) -> &'static str {
            "budget-lines"
        }
        fn supports_streaming_build(&self) -> bool {
            !self.core_jsonl
        }
        fn supports_bounded_json_lines_build(&self) -> bool {
            self.core_jsonl
        }

        fn frame(&self, bytes: &[u8]) -> Result<Vec<ByteRange>, String> {
            crate::source::json_lines::JsonLinesReader.frame(bytes)
        }

        fn parse(&self, bytes: &[u8]) -> Result<ExtractedRecord, String> {
            if bytes.starts_with(b"{\"id\":\"oldest") && self.gate.swap(false, Ordering::AcqRel) {
                self.entered.send(()).map_err(|error| error.to_string())?;
                self.released
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(10))
                    .map_err(|error| error.to_string())?;
            }
            crate::source::json_lines::JsonLinesReader.parse(bytes)
        }

        fn stream_build_chunks(
            &self,
            input: &mut dyn Read,
            _: usize,
            _: usize,
            emit: &mut dyn FnMut(BuildChunk) -> Result<(), String>,
        ) -> Option<Result<(), String>> {
            Some((|| {
                let mut bytes = Vec::new();
                input
                    .read_to_end(&mut bytes)
                    .map_err(|error| error.to_string())?;
                let late_failure = bytes.starts_with(b"{\"id\":\"late");
                let ranges = self.frame(&bytes)?;
                emit(BuildChunk {
                    bytes,
                    ranges,
                    file_offset: 0,
                    preceding_lines: 0,
                })?;
                if late_failure {
                    return Err("deliberate late stream failure".into());
                }
                Ok(())
            })())
        }
    }

    for (workers, core_jsonl, size) in [
        (4, false, 4096),
        (4, true, 4096),
        (1, false, 2 * 1024 * 1024),
    ] {
        let root = test_root("stream-budget");
        let (entered, gate) = mpsc::sync_channel(1);
        let (release, released) = mpsc::channel();
        let reader = Arc::new(BudgetedLines {
            core_jsonl,
            gate: AtomicBool::new(true),
            entered,
            released: Mutex::new(released),
        });
        let mut sources = crate::source::registry::SourceRegistry::builtin();
        sources
            .register("budget-lines", 1, "budget", reader)
            .unwrap();
        let mut builder =
            Index::builder()
                .with_source_registry(sources)
                .with_options(RepositoryOptions {
                    build_workers: std::num::NonZeroUsize::new(workers),
                    ..Default::default()
                });
        let collection = builder.collection("entries", Schema::default()).unwrap();
        let mut definition = CollectionDefinition::folder(
            "entries",
            "data",
            SourceFormat::registered("budget-lines"),
        );
        definition.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
        builder
            .register_file_source(collection, definition.file_source())
            .unwrap();
        builder
            .register_file_catalog(crate::files::FileCatalog::new(&root).unwrap())
            .unwrap();
        let index = builder.build().unwrap();
        let view = index.view().unwrap();
        let budget = &view.repository().stream_budget;
        let allowed_framers = workers.min(2);
        budget.set_test_limits(allowed_framers, 1024);
        let write = |name: &str, id: &str| {
            let path = root.join(format!("data/{name}.budget"));
            let mut data = format!("{{\"id\":\"{id}\"}}").into_bytes();
            data.resize(size, b' ');
            data.push(b'\n');
            std::fs::write(&path, data).unwrap();
            path
        };
        let first = write("first", "oldest");
        let others = (0..5)
            .map(|n| write(&format!("n{n}"), &format!("id{n}")))
            .collect::<Vec<_>>();
        std::thread::scope(|scope| {
            let oldest = scope.spawn(|| index.add_file(&first));
            let entered = gate.recv_timeout(Duration::from_secs(5));
            let imports = others
                .iter()
                .map(|path| scope.spawn(|| index.add_file(path)))
                .collect::<Vec<_>>();
            let mut current = budget.current();
            let deadline = Instant::now() + Duration::from_secs(5);
            while entered.is_ok()
                && workers > 1
                && (current.active_framers < 2 || current.pending_input_bytes < size as u64)
                && Instant::now() < deadline
            {
                std::thread::sleep(Duration::from_millis(2));
                current = budget.current();
            }
            let live_during_hold = index
                .view()
                .unwrap()
                .get_record("entries", "oldest", ReturnMode::Locator)
                .unwrap()
                .is_some();
            let _ = release.send(());
            entered.expect("oldest decoder did not reach its gate");
            assert_eq!(current.active_framers, allowed_framers as u64);
            assert_eq!(current.active_decoders, 1);
            assert!(current.admitted_decoder_bytes >= size as u64);
            if workers > 1 {
                assert!(current.pending_input_bytes >= size as u64);
                assert!(current.pending_and_admitted_bytes >= 2 * size as u64);
            }
            assert!(!live_during_hold);
            oldest.join().unwrap().unwrap();
            for import in imports {
                import.join().unwrap().unwrap();
            }
        });
        let peak = budget.peak();
        assert_eq!(peak.active_framers, allowed_framers as u64);
        if core_jsonl {
            assert_eq!(peak.active_callbacks, 0);
        } else {
            assert!((1..=allowed_framers as u64).contains(&peak.active_callbacks));
        }
        assert!(peak.completed_chunks > 0);
        assert!(peak.private_segments > 0);
        assert_eq!(budget.current(), Default::default());
        if !core_jsonl {
            let bad = write("retry", "late");
            assert!(index.add_file(&bad).is_err());
            assert!(
                index
                    .view()
                    .unwrap()
                    .file_handle_for_path(std::path::Path::new("data/retry.budget"))
                    .is_none()
            );
            assert_eq!(budget.current(), Default::default());
            assert_eq!(write("retry", "retried"), bad);
            index.add_file(&bad).unwrap();
            assert_eq!(budget.current(), Default::default());
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn verified_replacement_rejects_a_concurrent_replacement_of_the_same_handle() {
    use std::sync::mpsc;
    use std::time::Duration;

    let root = test_root("concurrent-verified-replace");
    let path = root.join("data/entry.jsonl");
    let old = b"{\"id\":\"old\"}\n";
    std::fs::write(&path, old).unwrap();
    let index = Index::start_build(
        &root,
        vec![intrinsic_definition("entries")],
        RepositoryOptions::default(),
        crate::source::registry::SourceRegistry::builtin(),
    )
    .unwrap()
    .wait_complete()
    .unwrap();
    let pinned = index.view().unwrap();
    let handle = pinned
        .file_handle_for_path(std::path::Path::new("data/entry.jsonl"))
        .unwrap();
    std::fs::write(&path, b"{\"id\":\"new\"}\n").unwrap();
    let (entered, arrival) = mpsc::sync_channel(1);
    let (release, resumed) = mpsc::channel::<()>();
    std::thread::scope(|scope| {
        let index_ref = &index;
        let path_ref = &path;
        let paused = scope.spawn(move || {
            FILE_PREPARATION_PROBE.with_borrow_mut(|probe| {
                *probe = Some(Box::new(move |phase| {
                    if phase == "old-layout" {
                        let _ = entered.send(());
                        let _ = resumed.recv_timeout(Duration::from_secs(10));
                    }
                    false
                }));
            });
            index_ref.update_file_with_options(
                handle,
                path_ref,
                UpdateOptions {
                    strategy: UpdateStrategy::PreferVerified,
                    old_content: Some(old),
                },
            )
        });
        let arrived = arrival.recv_timeout(Duration::from_secs(5));
        let current = index.update_file(handle, &path);
        let _ = release.send(());
        arrived.expect("verified reconstruction checkpoint");
        let current = current.unwrap();
        assert!(matches!(
            paused.join().unwrap(),
            Err(IndexError::FileConflict { .. })
        ));
        assert_eq!(index.view().unwrap().revision(), current.revision);
    });
    assert!(
        pinned
            .get_record("entries", "old", ReturnMode::Locator)
            .unwrap()
            .is_some()
    );
    assert!(
        index
            .view()
            .unwrap()
            .get_record("entries", "new", ReturnMode::Locator)
            .unwrap()
            .is_some()
    );
    std::fs::remove_dir_all(root).unwrap();
}
