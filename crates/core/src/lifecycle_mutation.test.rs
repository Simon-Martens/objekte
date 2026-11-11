use super::{
    FileMutationKind, FileMutationStrategy, Index, IndexError, UpdateOptions, UpdateStrategy,
    VerifiedFallbackReason,
};
use crate::inventory::SourceRevision;
use crate::lifecycle::DeleteStrategy;
use crate::model::{CollectionId, FieldPath, LogicalType, RecordSlot};
use crate::query::{CaseSensitivity, FtsMode, FtsQuery, Query, QueryOptions, parse_filter};
use crate::reference::{FieldDefinition, Schema};
use crate::relation::{RelationCardinality, RelationDefinition};
use crate::repository::{CollectionDefinition, IdentityPolicy, RepositoryOptions};
use crate::source::SourceFormat;
use crate::workload::{ReturnMode, SortField};

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
    Index::build(
        root,
        vec![CollectionDefinition::folder(
            "items",
            "data",
            SourceFormat::JsonLines,
        )],
        RepositoryOptions::default(),
    )
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
fn search_trusts_recorded_json_validation_until_explicit_update() {
    use crate::inventory::RecordClass;
    use std::time::{Duration, UNIX_EPOCH};

    let root = test_root("recorded-validation");
    let path = root.join("data/item.json");
    std::fs::write(&path, b"{\"x\":2,\"junk\":0}").unwrap();
    let index = Index::build(
        &root,
        vec![CollectionDefinition::folder(
            "items",
            "data",
            SourceFormat::JsonDecodedScalars,
        )],
        RepositoryOptions::default(),
    )
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
    // The stored classification remains valid even if the trusted extractor
    // encounters malformed bytes while walking the requested value.
    assert!(index.view().unwrap().search(&query).is_err());
    query.filter = None;
    assert_eq!(
        index
            .view()
            .unwrap()
            .get_record("items", &record_id, ReturnMode::Bytes)
            .unwrap()
            .unwrap()
            .record_class,
        RecordClass::Logical,
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
    let rows = |result: crate::repository::SearchResult| {
        result
            .hits
            .into_iter()
            .map(|hit| {
                (
                    hit.collection,
                    hit.record_id,
                    hit.file_path,
                    hit.file_status,
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
    use crate::repository::Payload;

    let root = test_root("move-same-metadata");
    let original = root.join("data/original.jsonl");
    let old_bytes = b"{\"id\":\"same\",\"name\":\"before\"}\n";
    let new_bytes = b"{\"id\":\"same\",\"name\":\"after!\"}\n";
    assert_eq!(old_bytes.len(), new_bytes.len());
    std::fs::write(&original, old_bytes).unwrap();
    let index = Index::build(
        &root,
        vec![intrinsic_definition("items")],
        RepositoryOptions::default(),
    )
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
    let index = Index::build(&root, vec![left, right], RepositoryOptions::default()).unwrap();
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
            .file_path,
        std::path::Path::new("data/right/item.jsonl")
    );
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();

    let root = test_root("move-reader");
    let original = root.join("data/item.jsonl");
    std::fs::write(&original, b"{\"id\":\"same\"}\n").unwrap();
    let mut json = intrinsic_definition("json");
    json.format = SourceFormat::Json;
    let index = Index::build(
        &root,
        vec![intrinsic_definition("jsonl"), json],
        RepositoryOptions::default(),
    )
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
    let index = Index::build(&root, vec![definition], RepositoryOptions::default()).unwrap();
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
    use crate::repository::Payload;

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
    let index = Index::build(
        &root,
        vec![definition.clone()],
        RepositoryOptions::default(),
    )
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
        old_hit.file_path,
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
                .map(|hit| hit.file_path.as_path())
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
                .all(|hit| hit.file_path == std::path::Path::new("data/z-moved.jsonl"))
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
                .all(|hit| hit.file_path == std::path::Path::new("data/z-moved.jsonl"))
        );
        let direct = view
            .get_record("items", "A", ReturnMode::Bytes)
            .unwrap()
            .unwrap();
        assert_eq!(direct.file_path, std::path::Path::new("data/z-moved.jsonl"));
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
    assert_eq!(
        SourceRevision::from_metadata(&std::fs::metadata(&original).unwrap()).unwrap(),
        SourceRevision::from_metadata(&original_metadata).unwrap()
    );
    assert_ne!(
        std::fs::metadata(&original).unwrap().created().ok(),
        original_metadata.created().ok()
    );
    let old_read = pinned
        .get_record("items", "A", ReturnMode::Bytes)
        .unwrap()
        .unwrap();
    assert_eq!(
        old_read.payload,
        Some(Payload::Bytes(
            b"{\"id\":\"X\",\"name\":\"xxxxx\"}".to_vec()
        ))
    );
    let old_page = pinned.search(&query(None, ReturnMode::Bytes)).unwrap();
    assert!(
        old_page
            .hits
            .iter()
            .any(|hit| hit.payload == old_read.payload)
    );
    std::fs::remove_file(&original).unwrap();

    let cache = root.join("cache.bin");
    index.flush_cache(&cache).unwrap();
    let restored = Index::load_cache(
        &root,
        vec![definition],
        RepositoryOptions::default(),
        &cache,
    )
    .unwrap();
    check_current(&restored);
    restored.compact().unwrap();
    check_current(&restored);
    restored.wait_for_reconciliation().unwrap();
    check_current(&restored);
    restored.shutdown().unwrap();
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
        let index = Index::build(
            &root,
            vec![intrinsic_definition("items")],
            RepositoryOptions::default(),
        )
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
fn restored_shared_partition_verified_update_preserves_its_record_slot() {
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
    let built = Index::build(
        &root,
        vec![definition.clone()],
        RepositoryOptions::default(),
    )
    .unwrap();
    let cache = root.join("cache.bin");
    built.flush_cache(&cache).unwrap();
    built.shutdown().unwrap();
    let restored = Index::load_cache(
        &root,
        vec![definition],
        RepositoryOptions::default(),
        &cache,
    )
    .unwrap();
    restored.wait_for_reconciliation().unwrap();
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
    let old_ordinal = old_shard.ordinal;
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
    assert_eq!(new_shard.ordinal, old_ordinal);
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
    let index = Index::build(
        &root,
        vec![intrinsic_definition("items")],
        RepositoryOptions::default(),
    )
    .unwrap();
    let path = root.join("data/verified.jsonl");
    let old = b"{\"id\":\"same\",\"name\":\"old\"}\n";
    std::fs::write(&path, old).unwrap();
    let added = index.add_file(&path).unwrap();
    let pinned = index.view().unwrap();
    let old_ordinal = pinned.segments(CollectionId(0)).unwrap()[0].ordinal;

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
        index.view().unwrap().segments(CollectionId(0)).unwrap()[0].ordinal,
        old_ordinal
    );
    assert_eq!(
        pinned.segments(CollectionId(0)).unwrap()[0].ordinal,
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
    assert_eq!(
        current.repository().collections()[0]
            .generation
            .records
            .len(),
        0
    );
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
    let cache = root.join("cache.bin");
    index.flush_cache(&cache).unwrap();
    let loaded = Index::load_cache(
        &root,
        vec![CollectionDefinition::folder(
            "items",
            "data",
            SourceFormat::JsonLines,
        )],
        RepositoryOptions::default(),
        &cache,
    )
    .unwrap();
    assert_eq!(
        loaded.view().unwrap().file_fingerprint(handle.file_id),
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
    assert_eq!(
        current.repository().collections()[0]
            .generation
            .records
            .len(),
        0
    );
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
    let index = Index::build(
        &root,
        vec![
            CollectionDefinition::folder("left", "data", SourceFormat::JsonLines),
            CollectionDefinition::folder("right", "other", SourceFormat::JsonLines),
        ],
        RepositoryOptions::default(),
    )
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
    let index = Index::build(&root, definitions.clone(), RepositoryOptions::default()).unwrap();
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

    let cache = root.join("cache.bin");
    assert_eq!(index.flush_cache(&cache).unwrap(), current.revision());
    let restored =
        Index::load_cache(&root, definitions, RepositoryOptions::default(), &cache).unwrap();
    let loaded = restored.view().unwrap();
    assert_eq!(loaded.revision(), current.revision());
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
    restored.wait_for_reconciliation().unwrap();
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
    let old_ordinal = index.view().unwrap().segments(CollectionId(0)).unwrap()[0].ordinal;
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
        current.segments(CollectionId(0)).unwrap()[0].ordinal,
        old_ordinal
    );
    assert_eq!(index.status().unwrap().live_records, 1);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn one_record_shared_file_preserves_every_collection_slot_atomically() {
    let root = test_root("shared-slot");
    let index = Index::build(
        &root,
        vec![intrinsic_definition("left"), intrinsic_definition("right")],
        RepositoryOptions::default(),
    )
    .unwrap();
    let path = root.join("data/shared.jsonl");
    let old = b"{\"id\":\"same\",\"name\":\"old\"}\n";
    std::fs::write(&path, old).unwrap();
    let added = index.add_file(&path).unwrap();
    let pinned = index.view().unwrap();
    let ordinals = [CollectionId(0), CollectionId(1)]
        .map(|collection| pinned.segments(collection).unwrap()[0].ordinal);
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
        assert_eq!(shards[0].ordinal, ordinal);
        assert_eq!(shards[0].index.records[0].slot, RecordSlot(0));
        assert_eq!(pinned.segments(collection).unwrap()[0].ordinal, ordinal);
    }
    assert_eq!(index.status().unwrap().live_records, 2);
    std::fs::remove_dir_all(root).unwrap();
}
