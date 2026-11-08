use super::{
    FileMutationKind, FileMutationStrategy, Index, IndexError, UpdateOptions, UpdateStrategy,
    VerifiedFallbackReason,
};
use crate::inventory::SourceRevision;
use crate::lifecycle::DeleteStrategy;
use crate::model::CollectionId;
use crate::query::{Query, QueryOptions};
use crate::repository::{CollectionDefinition, RepositoryOptions};
use crate::source::SourceFormat;
use crate::workload::ReturnMode;

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
    assert_eq!(moved.retired_records, 2);
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
fn verified_update_removes_old_shard_and_bad_hint_falls_back() {
    let root = test_root("verified");
    let index = index(&root);
    let path = root.join("data/verified.jsonl");
    let old = b"{\"name\":\"old\"}\n";
    std::fs::write(&path, old).unwrap();
    let added = index.add_file(&path).unwrap();
    let pinned = index.view().unwrap();
    let old_ordinal = pinned.segments(CollectionId(0)).unwrap()[0].ordinal;

    let new = b"{\"name\":\"new\"}\n";
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
        FileMutationStrategy::VerifiedPhysicalRemoval
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
    assert_ne!(
        index.view().unwrap().segments(CollectionId(0)).unwrap()[0].ordinal,
        old_ordinal
    );
    assert_eq!(
        pinned.segments(CollectionId(0)).unwrap()[0].ordinal,
        old_ordinal
    );
    assert_eq!(pinned.segments(CollectionId(0)).unwrap()[0].active.len(), 1);

    let latest = b"{\"name\":\"latest\"}\n";
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
fn verified_hint_on_unfingerprinted_base_uses_append_fallback() {
    let root = test_root("base-fallback");
    let path = root.join("data/base.jsonl");
    let old = b"{\"name\":\"old\"}\n";
    std::fs::write(&path, old).unwrap();
    let index = index(&root);
    let handle = index.view().unwrap().file_handle_for_path(&path).unwrap();
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
    assert_eq!(updated.strategy, FileMutationStrategy::AppendReplacement);
    assert_eq!(
        updated.fallback_reason,
        Some(VerifiedFallbackReason::MissingFingerprint)
    );
    assert_eq!(index.status().unwrap().live_records, 1);
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
